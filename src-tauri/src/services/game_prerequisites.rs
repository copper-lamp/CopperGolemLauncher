//! 游戏系统依赖：只从目标 PE 和包清单声明提取要求，不按游戏版本猜测。
use std::{fs,path::{Path,PathBuf}};
use serde::Serialize;
use crate::error::KernelError;
#[derive(Clone,Debug,Serialize,PartialEq,Eq)]
#[serde(rename_all="camelCase")]
pub struct PackageRequirement {pub name:String,pub publisher:String,pub min_version:String,pub architecture:String}
#[derive(Clone,Debug,Serialize)]
#[serde(rename_all="camelCase")]
pub struct GameRequirements {pub architecture:String,pub imported_runtime_dlls:Vec<String>,pub package_dependencies:Vec<PackageRequirement>}
fn invalid()->KernelError{KernelError::InvalidArgument("game_dependency_metadata_invalid".into())}
fn u16at(data:&[u8],off:usize)->Result<u16,KernelError>{Ok(u16::from_le_bytes(data.get(off..off+2).ok_or_else(invalid)?.try_into().map_err(|_|invalid())?))}
fn u32at(data:&[u8],off:usize)->Result<u32,KernelError>{Ok(u32::from_le_bytes(data.get(off..off+4).ok_or_else(invalid)?.try_into().map_err(|_|invalid())?))}
/// PE 导入目录；不加载目标 DLL，也不执行任何游戏代码。
pub fn pe_requirements(data:&[u8])->Result<(String,Vec<String>),KernelError>{
    if data.get(..2)!=Some(b"MZ"){return Err(invalid());}let pe=u32at(data,0x3c)? as usize;
    if data.get(pe..pe+4)!=Some(b"PE\0\0"){return Err(invalid());}
    let arch=match u16at(data,pe+4)?{0x8664=>"x64",0xaa64=>"arm64",0x14c=>"x86",_=>return Err(invalid())}.to_owned();
    let count=u16at(data,pe+6)? as usize;if count>96{return Err(invalid());}
    let optional=pe+24;let opt_size=u16at(data,pe+20)? as usize;
    let directories=match u16at(data,optional)?{0x20b=>optional+112,0x10b=>optional+96,_=>return Err(invalid())};
    if directories+16>optional+opt_size{return Err(invalid());}
    let section_start=optional+opt_size;let mut sections=Vec::new();
    for i in 0..count{let p=section_start+i*40;sections.push((u32at(data,p+12)?,u32at(data,p+8)?.max(u32at(data,p+16)?),u32at(data,p+20)?,u32at(data,p+16)?));}
    let offset=|rva:u32|->Result<usize,KernelError>{for &(start,size,raw,raw_size) in &sections{if rva>=start && rva-start<size{let delta=rva-start;if delta>=raw_size{return Err(invalid());}return (raw as usize).checked_add(delta as usize).filter(|&p|p<data.len()).ok_or_else(invalid);}}Err(invalid())};
    let import=u32at(data,directories+8)?;let size=u32at(data,directories+12)?;
    let mut names=Vec::new();if import==0{return Ok((arch,names));}let start=offset(import)?;
    let mut terminated=false;
    for i in 0..(size as usize/20).min(4096){let p=start+i*20;let name=u32at(data,p+12)?;if name==0 {terminated=true;break;}
        let n=offset(name)?;let tail=data.get(n..).ok_or_else(invalid)?;let len=tail.iter().take(512).position(|&b|b==0).ok_or_else(invalid)?;
        let name=std::str::from_utf8(&tail[..len]).map_err(|_|invalid())?.to_ascii_lowercase();
        if name.chars().any(|c|!c.is_ascii_alphanumeric()&&c!='.'&&c!='_'&&c!='-'){return Err(invalid());}
        if name.starts_with("vcruntime140")||name.starts_with("msvcp140")||name.starts_with("concrt140") {names.push(name);}
    }
    if !terminated{return Err(invalid());}names.sort();names.dedup();Ok((arch,names))
}
pub fn manifest_requirements(bytes:&[u8])->Result<Vec<PackageRequirement>,KernelError>{
    use quick_xml::{Reader,events::Event};
    let mut reader=Reader::from_reader(bytes);let mut stack=Vec::new();let mut requirements=Vec::new();
    loop{let (e,empty)=match reader.read_event(){Ok(Event::Start(e))=>(e,false),Ok(Event::Empty(e))=>(e,true),Ok(Event::End(_))=>{stack.pop();continue;},Ok(Event::Eof)=>break,Err(_)=>return Err(invalid()),_=>continue};{
        if e.local_name().as_ref()!=b"PackageDependency" || stack.last().map(String::as_str)!=Some("Dependencies"){if !empty{stack.push(String::from_utf8_lossy(e.local_name().as_ref()).into_owned());}continue;}
        let mut requirement=PackageRequirement{name:String::new(),publisher:String::new(),min_version:"0.0.0.0".into(),architecture:"neutral".into()};
        for attribute in e.attributes(){let a=attribute.map_err(|_|invalid())?;let v=a.decode_and_unescape_value(reader.decoder()).map_err(|_|invalid())?.into_owned();match a.key.as_ref(){b"Name"=>requirement.name=v,b"Publisher"=>requirement.publisher=v,b"MinVersion"=>requirement.min_version=v,b"ProcessorArchitecture"=>requirement.architecture=v,_=>{}}}
        if requirement.name.is_empty(){return Err(invalid());}requirements.push(requirement);
        if !empty{stack.push("PackageDependency".into());}
    }}
    Ok(requirements)
}
pub fn inspect(dir:&Path,exe:&Path)->Result<GameRequirements,KernelError>{
    let meta=fs::metadata(exe)?;if meta.len()>1024*1024*1024{return Err(invalid());}
    let (architecture,imported_runtime_dlls)=pe_requirements(&fs::read(exe)?)?;
    let manifest=dir.join("AppxManifest.xml");let package_dependencies=if manifest.is_file(){let size=fs::metadata(&manifest)?.len();if size>4*1024*1024{return Err(invalid());}manifest_requirements(&fs::read(manifest)?)?}else{Vec::new()};
    Ok(GameRequirements{architecture,imported_runtime_dlls,package_dependencies})
}
/// 明确的目录列表；不用搜索 PATH，且验证 DLL 的目标架构。
pub fn missing_vc_dlls(requirements:&GameRequirements,game_dir:&Path)->Vec<String>{
    #[cfg(windows)] let system=std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(||PathBuf::from("C:/Windows")).join(if requirements.architecture=="x86"&&cfg!(target_arch="x86_64"){"SysWOW64"}else{"System32"});
    #[cfg(not(windows))] let system=PathBuf::from("/nonexistent");
    requirements.imported_runtime_dlls.iter().filter(|name| ![game_dir.join(name),system.join(name)].iter().any(|p|fs::read(p).ok().and_then(|b|pe_requirements(&b).ok()).is_some_and(|(a,_)|a==requirements.architecture))).cloned().collect()
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn package_requirements_come_from_declared_dependencies(){let xml=br#"<Package><Dependencies><PackageDependency Name="Microsoft.VCLibs.140.00.UWPDesktop" Publisher="CN=Microsoft Corporation" MinVersion="14.0.30704.0" ProcessorArchitecture="x64"/></Dependencies><Other><PackageDependency Name="Ignore"/></Other></Package>"#;let r=manifest_requirements(xml).unwrap();assert_eq!(r.len(),1);assert_eq!(r[0].name,"Microsoft.VCLibs.140.00.UWPDesktop");assert_eq!(r[0].min_version,"14.0.30704.0");}
 #[test]fn paired_manifest_elements_are_supported(){let xml=br#"<Package><Dependencies><PackageDependency Name="Microsoft.VCLibs.140.00" MinVersion="14.0.0.0"></PackageDependency></Dependencies></Package>"#;assert_eq!(manifest_requirements(xml).unwrap()[0].name,"Microsoft.VCLibs.140.00");}
 #[test]fn malformed_pe_is_rejected_without_loading(){assert!(pe_requirements(b"MZ").is_err());assert!(pe_requirements(&vec![0;1024]).is_err());}
 #[test]fn extracts_vc_import_and_architecture(){let mut b=vec![0u8;1024];b[..2].copy_from_slice(b"MZ");b[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());b[64..68].copy_from_slice(b"PE\0\0");b[68..70].copy_from_slice(&0x8664u16.to_le_bytes());b[70..72].copy_from_slice(&1u16.to_le_bytes());b[84..86].copy_from_slice(&240u16.to_le_bytes());b[88..90].copy_from_slice(&0x20bu16.to_le_bytes());b[208..212].copy_from_slice(&0x1000u32.to_le_bytes());b[212..216].copy_from_slice(&40u32.to_le_bytes());let s=328;b[s+8..s+12].copy_from_slice(&512u32.to_le_bytes());b[s+12..s+16].copy_from_slice(&0x1000u32.to_le_bytes());b[s+16..s+20].copy_from_slice(&512u32.to_le_bytes());b[s+20..s+24].copy_from_slice(&512u32.to_le_bytes());b[524..528].copy_from_slice(&0x1040u32.to_le_bytes());b[576..593].copy_from_slice(b"VCRUNTIME140.dll\0");let (arch,dlls)=pe_requirements(&b).unwrap();assert_eq!(arch,"x64");assert_eq!(dlls,vec!["vcruntime140.dll"]);}
}
