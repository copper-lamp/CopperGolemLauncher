//! Windows 系统运行库安装；仅微软签名安装器、显式 UAC、重启状态独立返回。
use std::path::Path;
use serde::Serialize;
use crate::error::KernelError;
use super::game_prerequisites::{self,GameRequirements,PackageRequirement};
fn windows_version_at_least(actual:&str,minimum:&str)->bool {
    fn parse(value:&str)->Option<[u16;4]>{let numbers=value.split('.').map(str::parse::<u16>).collect::<Result<Vec<_>,_>>().ok()?;numbers.try_into().ok()}
    match (parse(actual),parse(minimum)){(Some(a),Some(m))=>a>=m,_=>false}
}
#[cfg(test)]mod tests{use super::*;#[test]fn package_version_uses_numeric_four_part_comparison(){assert!(windows_version_at_least("14.0.33519.0","14.0.30704.0"));assert!(!windows_version_at_least("14.0.9.0","14.0.10.0"));assert!(!windows_version_at_least("14.0.1","14.0.0.0"));}}
#[derive(Clone,Debug,Serialize)]
#[serde(rename_all="camelCase")]
pub struct SystemDependencyStatus {pub requirements:GameRequirements,pub missing_vc_dlls:Vec<String>,pub missing_packages:Vec<PackageRequirement>,pub state:String}
#[derive(Clone,Debug,Serialize)]
#[serde(rename_all="camelCase")]
pub struct SystemInstallOutcome {pub state:String,pub exit_code:Option<i32>}
#[cfg(windows)]
async fn powershell(script:&str,values:&[(&str,String)])->Result<std::process::Output,KernelError>{
    use base64::Engine;
    let bytes:Vec<u8>=script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded=base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut command=tokio::process::Command::new("powershell.exe");command.args(["-NoProfile","-NonInteractive","-EncodedCommand",&encoded]).kill_on_drop(true).creation_flags(0x08000000);
    for (key,value) in values{command.env(key,value);}
    tokio::time::timeout(std::time::Duration::from_secs(600),command.output()).await.map_err(|_|KernelError::Module("system_dependency_timeout".into()))?.map_err(Into::into)
}
#[cfg(windows)]
pub async fn probe(dir:&Path,exe:&Path)->Result<SystemDependencyStatus,KernelError>{
    let requirements=game_prerequisites::inspect(dir,exe)?;
    let missing_vc_dlls=game_prerequisites::missing_vc_dlls(&requirements,dir);let mut missing_packages=Vec::new();
    for requirement in &requirements.package_dependencies{
        let output=powershell("$ErrorActionPreference='Stop'; $p=Get-AppxPackage -Name $env:COPPER_PACKAGE_NAME | Where-Object { $_.Publisher -eq $env:COPPER_PACKAGE_PUBLISHER -and [version]$_.Version -ge [version]$env:COPPER_PACKAGE_VERSION -and ($env:COPPER_PACKAGE_ARCH -eq 'neutral' -or $_.Architecture.ToString() -eq $env:COPPER_PACKAGE_ARCH) }; if($p){exit 0}else{exit 2}",&[("COPPER_PACKAGE_NAME",requirement.name.clone()),("COPPER_PACKAGE_PUBLISHER",requirement.publisher.clone()),("COPPER_PACKAGE_VERSION",requirement.min_version.clone()),("COPPER_PACKAGE_ARCH",requirement.architecture.clone())]).await?;
        match output.status.code(){Some(0)=>{},Some(2)=>missing_packages.push(requirement.clone()),_=>return Err(KernelError::Module("system_dependency_probe_failed".into()))}
    }
    let state=if missing_vc_dlls.is_empty()&&missing_packages.is_empty(){"ready"}else{"missing"}.into();
    Ok(SystemDependencyStatus{requirements,missing_vc_dlls,missing_packages,state})
}
#[cfg(not(windows))]
pub async fn probe(_dir:&Path,_exe:&Path)->Result<SystemDependencyStatus,KernelError>{Err(KernelError::Module("system_dependency_platform_unsupported".into()))}
#[cfg(windows)]
pub async fn install_vc(status:&SystemDependencyStatus,cache:&Path)->Result<SystemInstallOutcome,KernelError>{
    if status.missing_vc_dlls.is_empty(){return Ok(SystemInstallOutcome{state:if status.missing_packages.is_empty(){"ready"}else{"package-dependency-requires-store"}.into(),exit_code:None});}
    let architecture=&status.requirements.architecture;
    if !matches!(architecture.as_str(),"x64"|"x86"|"arm64"){return Err(KernelError::InvalidArgument("system_dependency_architecture".into()));}
    let url=format!("https://aka.ms/vc14/vc_redist.{architecture}.exe");
    let client=super::http_client::client_builder(std::time::Duration::from_secs(180)).redirect(reqwest::redirect::Policy::custom(|a|{
        let u=a.url();if a.previous().len()>8||u.scheme()!="https"||!matches!(u.host_str(),Some("aka.ms"|"download.visualstudio.microsoft.com"|"download.microsoft.com")){a.error("system_dependency_source_rejected")}else{a.follow()}
    })).build().map_err(|e|KernelError::Module(e.to_string()))?;
    let mut response=client.get(url).send().await?.error_for_status()?;let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await?{if bytes.len()+chunk.len()>100*1024*1024{return Err(KernelError::Module("system_dependency_download_limit".into()));}bytes.extend_from_slice(&chunk);}
    std::fs::create_dir_all(cache)?;let file=cache.join(format!("vc-redist-{}.exe",uuid::Uuid::new_v4()));std::fs::write(&file,bytes)?;
    // 脚本固定，所有路径/包元数据走环境变量，绝不拼接进可执行语法。
    let result=powershell("$ErrorActionPreference='Stop'; $sig=Get-AuthenticodeSignature -LiteralPath $env:COPPER_INSTALLER; if($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch '(^|, )O=Microsoft Corporation(,|$)'){exit 125}; try{$p=Start-Process -FilePath $env:COPPER_INSTALLER -ArgumentList '/install','/passive','/norestart' -Verb RunAs -Wait -PassThru; exit $p.ExitCode}catch{if($_.Exception.NativeErrorCode -eq 1223){exit 1223};exit 126}",&[("COPPER_INSTALLER",file.to_string_lossy().into_owned())]).await;
    let _=std::fs::remove_file(file);
    let code=result?.status.code();let state=match code{Some(0)=>"installed",Some(3010)|Some(1641)=>"restart-required",Some(1223)=>"elevation-cancelled",Some(125)=>"signature-rejected",Some(1638)=>"already-installed",_=>"failed"};
    Ok(SystemInstallOutcome{state:state.into(),exit_code:code})
}
/// 已有微软官方直链的框架包；未知包身份拒绝推测地址。
#[cfg(windows)]
pub async fn install_packages(status:&SystemDependencyStatus,cache:&Path)->Result<SystemInstallOutcome,KernelError>{
    for requirement in &status.missing_packages {
        if requirement.name!="Microsoft.VCLibs.140.00.UWPDesktop" || requirement.publisher!="CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US" {return Ok(SystemInstallOutcome{state:"package-source-unsupported".into(),exit_code:None});}
        let arch=if requirement.architecture=="neutral"{&status.requirements.architecture}else{&requirement.architecture};
        if !matches!(arch.as_str(),"x86"|"x64"|"arm64"){return Err(KernelError::InvalidArgument("system_dependency_architecture".into()));}
        let client=super::http_client::client_builder(std::time::Duration::from_secs(180)).redirect(reqwest::redirect::Policy::custom(|a|{let u=a.url();if a.previous().len()>8||u.scheme()!="https"||!matches!(u.host_str(),Some("aka.ms"|"download.microsoft.com"|"download.visualstudio.microsoft.com")){a.error("system_dependency_source_rejected")}else{a.follow()}})).build().map_err(|e|KernelError::Module(e.to_string()))?;
        let mut response=client.get(format!("https://aka.ms/Microsoft.VCLibs.{arch}.14.00.Desktop.appx")).send().await?.error_for_status()?;
        let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await?{if bytes.len()+chunk.len()>100*1024*1024{return Err(KernelError::Module("system_dependency_download_limit".into()));}bytes.extend_from_slice(&chunk);}
        // 在系统部署前验证下载包的声明身份/发布者/架构，系统Add-AppxPackage负责签名信任链验证。
        {
        let mut archive=zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice())).map_err(|_|KernelError::Module("system_dependency_package_invalid".into()))?;
        let mut manifest=archive.by_name("AppxManifest.xml").map_err(|_|KernelError::Module("system_dependency_package_invalid".into()))?;
        if manifest.size()>4*1024*1024{return Err(KernelError::Module("system_dependency_package_invalid".into()));}
        let mut xml=String::new();std::io::Read::read_to_string(&mut manifest,&mut xml)?;drop(manifest);drop(archive);
        let mut reader=quick_xml::Reader::from_str(&xml);let mut matched=false;
        loop {match reader.read_event(){Ok(quick_xml::events::Event::Empty(e))|Ok(quick_xml::events::Event::Start(e)) if e.local_name().as_ref()==b"Identity"=>{let mut name=String::new();let mut publisher=String::new();let mut architecture=String::new();let mut version=String::new();for a in e.attributes(){let a=a.map_err(|_|KernelError::Module("system_dependency_package_invalid".into()))?;let v=a.decode_and_unescape_value(reader.decoder()).map_err(|_|KernelError::Module("system_dependency_package_invalid".into()))?.into_owned();match a.key.as_ref(){b"Name"=>name=v,b"Publisher"=>publisher=v,b"ProcessorArchitecture"=>architecture=v,b"Version"=>version=v,_=>{}}}matched=name==requirement.name&&publisher==requirement.publisher&&architecture==*arch&&windows_version_at_least(&version,&requirement.min_version);break;},Ok(quick_xml::events::Event::Eof)=>break,Err(_)=>break,_=>{}}}
        if !matched{return Err(KernelError::Module("system_dependency_identity_mismatch".into()));}
        }
        std::fs::create_dir_all(cache)?;let path=cache.join(format!("vclibs-{}.appx",uuid::Uuid::new_v4()));std::fs::write(&path,bytes)?;
        let result=powershell("$ErrorActionPreference='Stop'; try{Add-AppxPackage -Path $env:COPPER_INSTALLER -ErrorAction Stop;exit 0}catch{exit 126}",&[("COPPER_INSTALLER",path.to_string_lossy().into_owned())]).await;
        let _=std::fs::remove_file(path);let code=result?.status.code();if code!=Some(0){return Ok(SystemInstallOutcome{state:"package-install-failed".into(),exit_code:code});}
    }
    Ok(SystemInstallOutcome{state:"installed".into(),exit_code:Some(0)})
}
#[cfg(not(windows))]
pub async fn install_packages(_status:&SystemDependencyStatus,_cache:&Path)->Result<SystemInstallOutcome,KernelError>{Err(KernelError::Module("system_dependency_platform_unsupported".into()))}
#[cfg(not(windows))]
pub async fn install_vc(_status:&SystemDependencyStatus,_cache:&Path)->Result<SystemInstallOutcome,KernelError>{Err(KernelError::Module("system_dependency_platform_unsupported".into()))}
