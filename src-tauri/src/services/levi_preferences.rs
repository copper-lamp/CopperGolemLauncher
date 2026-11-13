//! Read installed Levi preferences without scanning instances or modifying Levi files.
use std::{collections::BTreeMap,path::{Path,PathBuf},time::{Duration,Instant}};
use serde::Serialize;
#[derive(Default,Serialize)]
#[serde(rename_all="camelCase")]
pub struct Preferences { pub installations:Vec<Installation>,pub warnings:Vec<String> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct Installation { pub config_path:String,pub versions_directory:String,pub colors:Colors }
#[derive(Default,Serialize)]
pub struct Colors {#[serde(skip_serializing_if="Option::is_none")]pub light:Option<String>,#[serde(skip_serializing_if="Option::is_none")]pub dark:Option<String>}
fn regular(p:&Path)->bool {std::fs::symlink_metadata(p).is_ok_and(|m|{if m.file_type().is_symlink(){return false;}#[cfg(windows)]{use std::os::windows::fs::MetadataExt;if m.file_attributes()&0x400!=0{return false;}}true})}
fn color(map:&BTreeMap<String,String>,mode:&str)->Option<String>{
    let selected=map.get(&format!("app.{mode}ThemeColor")).or_else(||map.get("app.themeColor"))?;
    let raw=match selected.as_str(){"emerald"=>return None,"blue"=>"#3b82f6","violet"=>"#8b5cf6","amber"=>"#f59e0b","pink"|"rose"=>"#ff6b8b","cyan"=>"#06b6d4","win_yellow"=>"#ffb900","win_orange"=>"#ff8c00","win_red"=>"#d13438","win_blue"=>"#0078d7","win_green"=>"#10893e","win_purple"=>"#6b69d6","win_teal"=>"#0099bc","win_pink_red"=>"#e74856","custom"=>map.get(&format!("app.{mode}CustomThemeColor")).or_else(||map.get("app.customThemeColor"))?,_=>return None};
    if raw.len()!=7||!raw.starts_with('#')||!raw[1..].bytes().all(|b|b.is_ascii_hexdigit())||raw.eq_ignore_ascii_case("#10b981"){None}else{Some(raw.to_ascii_lowercase())}
}
/// Windows Levi stores config under APPDATA/<executable name>. Other platforms do not
/// have the Windows launcher/WebView2 profile; return empty instead of guessing paths.
pub fn read()->Preferences {
    let mut result=Preferences::default();
    if !cfg!(windows){return result;}
    let Some(appdata)=std::env::var_os("APPDATA").map(PathBuf::from)else{return result};
    let Ok(entries)=std::fs::read_dir(appdata)else{return result};let until=Instant::now()+Duration::from_secs(4);let mut candidates=Vec::new();
    for (count,entry) in entries.enumerate(){if count>=1024||Instant::now()>until{result.warnings.push("Levi configuration discovery reached its read budget".into());break;}let Ok(entry)=entry else{continue};let name=entry.file_name().to_string_lossy().to_ascii_lowercase();if name.contains("levi")&&name.ends_with(".exe")&&entry.file_type().is_ok_and(|t|t.is_dir())&&regular(&entry.path()){candidates.push(entry.path());if candidates.len()>=8{break;}}}
    candidates.sort();for root in candidates {if Instant::now()>until{result.warnings.push("Levi preference discovery reached its time budget".into());break;}let config=root.join("config.json");let metadata=match std::fs::symlink_metadata(&config){Ok(m)if m.is_file()&&m.len()<=65536&&regular(&config)=>m,_=>continue};let _=metadata;
        let parsed=(||->Result<serde_json::Value,String>{use std::io::Read;let mut bytes=Vec::new();std::fs::File::open(&config).map_err(|e|e.to_string())?.take(65537).read_to_end(&mut bytes).map_err(|e|e.to_string())?;if bytes.len()>65536{return Err("Levi config exceeds size budget".into());}serde_json::from_slice(&bytes).map_err(|e|e.to_string())})();let data=match parsed{Ok(v)=>v,Err(e)=>{result.warnings.push(format!("{}: {e}",config.display()));continue}};
        let base=data.get("base_root").and_then(|v|v.as_str()).filter(|s|!s.trim().is_empty()).map(PathBuf::from).unwrap_or_else(||root.clone());if !base.is_absolute(){result.warnings.push(format!("{}: relative base_root is unsupported",config.display()));continue;}let versions=base.join("versions");if !versions.is_dir()||!regular(&versions){continue;}
        let mut colors=Colors::default();let db=root.join("EBWebView/Default/Local Storage/leveldb");if db.is_dir(){if !regular(&root.join("EBWebView"))||!regular(&root.join("EBWebView/Default"))||!regular(&root.join("EBWebView/Default/Local Storage"))||!regular(&db){result.warnings.push("Levi WebView profile contains a link/reparse point; colors skipped".into());}else{match super::levi_leveldb::preferences(&db){Ok(map)=>{colors.light=color(&map,"light");colors.dark=color(&map,"dark");},Err(e)=>result.warnings.push(format!("Levi colors unavailable: {e}"))}}}
        result.installations.push(Installation{config_path:config.to_string_lossy().into(),versions_directory:versions.to_string_lossy().into(),colors});
    }result
}
