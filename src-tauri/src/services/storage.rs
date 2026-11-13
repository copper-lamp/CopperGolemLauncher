//! Restart-safe storage relocation. The locator lives outside the movable launcher DB.
use std::path::{Path,PathBuf};
use serde::{Deserialize,Serialize};
use rusqlite::OptionalExtension;
use sha2::{Digest,Sha256};
use crate::error::KernelError;
use super::paths::{Paths,ensure_writable_dir};
#[derive(Debug,Clone,Serialize,Deserialize,Default)]
#[serde(rename_all="camelCase")]
pub struct StorageRequest { pub launcher_root: Option<String>, pub game_root: Option<String> }
#[derive(Debug,Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
struct Plan { source: PathBuf, target: PathBuf, old_versions: PathBuf, old_packages: PathBuf, game_root: Option<PathBuf> }
fn db(dir:&Path)->Result<rusqlite::Connection,KernelError>{
    ensure_writable_dir(dir,"存储定位器")?;
    let c=rusqlite::Connection::open(dir.join("storage-locator.sqlite"))?;
    c.execute_batch("CREATE TABLE IF NOT EXISTS locator (key TEXT PRIMARY KEY,value TEXT NOT NULL)")?; Ok(c)
}
fn get(c:&rusqlite::Connection,key:&str)->Result<Option<String>,KernelError>{Ok(c.query_row("SELECT value FROM locator WHERE key=?1",[key],|r|r.get(0)).optional()?)}
fn set(c:&rusqlite::Connection,key:&str,value:&str)->Result<(),KernelError>{c.execute("INSERT INTO locator(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[key,value])?;Ok(())}
fn error(message:impl Into<String>)->KernelError{KernelError::Config(message.into())}
fn path(raw:&str)->Result<PathBuf,KernelError>{
    if raw.starts_with("content:") {return Err(error("SAF document URI 尚无可用的持久目录桥接；请选择应用私有目录或已授权的文件系统路径"));}
    let p=PathBuf::from(raw.trim());
    if !p.is_absolute(){return Err(error("存储位置必须是绝对路径"));}
    if p.components().any(|c|matches!(c,std::path::Component::ParentDir)){return Err(error("存储路径不能包含上级目录跳转"));}
    Ok(p)
}
fn linked(meta:&std::fs::Metadata)->bool{
    if meta.file_type().is_symlink(){return true;}
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if meta.file_attributes() & 0x400 != 0 {return true;} }
    false
}
fn reject_links(p:&Path)->Result<(),KernelError>{
    for ancestor in p.ancestors(){match std::fs::symlink_metadata(ancestor){Ok(m) if linked(&m)=>return Err(error(format!("存储路径含符号链接或重解析点: {}",ancestor.display()))),Ok(_)=>{},Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.into())}}
    Ok(())
}
fn separate(source:&Path,target:&Path)->Result<(),KernelError>{
    reject_links(source)?;reject_links(target)?;
    if source==target{return Ok(());}
    // Canonicalize existing ancestors, so case aliases on Windows cannot bypass overlap checks.
    fn normalized(p:&Path)->Result<PathBuf,KernelError>{if p.exists(){return Ok(p.canonicalize()?);}let parent=p.parent().ok_or_else(||error("无效存储根"))?;Ok(normalized(parent)?.join(p.file_name().ok_or_else(||error("无效存储根"))?))}
    let s=normalized(source)?;let t=normalized(target)?;
    if s==t||s.starts_with(&t)||t.starts_with(&s){return Err(error("源和目标目录不能重叠或互为父目录"));}Ok(())
}
fn empty_destination(p:&Path)->Result<(),KernelError>{
    reject_links(p)?;
    if p.exists() && (!p.is_dir()||std::fs::read_dir(p)?.next().is_some()){return Err(error(format!("目标必须为空目录，保留现有内容: {}",p.display())));}
    ensure_writable_dir(p,"迁移目标")
}
fn write_marker(dir:&Path,token:&str)->Result<(),KernelError>{use std::io::Write;let mut file=std::fs::File::create(dir.join(".cgl-migration-owner"))?;file.write_all(token.as_bytes())?;file.sync_all()?;Ok(())}
fn digest(p:&Path)->Result<Vec<u8>,KernelError>{use std::io::Read;let mut f=std::fs::File::open(p)?;let mut h=Sha256::new();let mut b=[0u8;65536];loop{let n=f.read(&mut b)?;if n==0{break;}h.update(&b[..n]);}Ok(h.finalize().to_vec())}
fn copy_tree(source:&Path,target:&Path)->Result<(),KernelError>{
    let meta=std::fs::symlink_metadata(source)?;if linked(&meta){return Err(error(format!("拒绝复制符号链接或重解析点: {}",source.display())));}
    if meta.is_dir(){std::fs::create_dir_all(target)?;for e in std::fs::read_dir(source)?{let e=e?;copy_tree(&e.path(),&target.join(e.file_name()))?;}}
    else if meta.is_file(){std::fs::copy(source,target)?;std::fs::OpenOptions::new().write(true).open(target)?.sync_all()?;if digest(source)?!=digest(target)?{return Err(error(format!("复制校验失败: {}",source.display())));}}
    else{return Err(error("迁移不支持设备或特殊文件"));}Ok(())
}
/// Called before Paths::prepare and before opening the application database.
pub fn bootstrap(locator:&Path,default:&Path,legacy:&Path,env:Option<PathBuf>)->Result<PathBuf,KernelError>{
    // Development overrides must work even if the platform app-data locator is read-only.
    if let Some(root)=env {return Ok(root);}
    let mut c=db(locator)?;
    let active=get(&c,"active")?.map(PathBuf::from);
    let initial=active.unwrap_or_else(||if legacy.join("data/copper.db").is_file(){legacy.to_owned()}else{default.to_owned()});
    if let Some(raw)=get(&c,"pending")?{
        let plan:Plan=serde_json::from_str(&raw)?;
        match execute(&plan){
            Ok(created)=>{
                let result=(||{let tx=c.transaction()?;set(&tx,"active",&plan.target.to_string_lossy())?;if let Some(game)=&plan.game_root{set(&tx,"gameRoot",&game.to_string_lossy())?;set(&tx,"resetVersionsSelection","1")?;}tx.execute("DELETE FROM locator WHERE key IN ('pending','lastError')",[])?;tx.commit()?;Ok::<(),KernelError>(())})();
                if let Err(e)=result{for p in created.iter().rev(){let _=std::fs::remove_dir_all(p);}return Err(e);}
                for p in &created {let _=std::fs::remove_file(p.join(".cgl-migration-owner"));}
                reset_versions_selection(&c,&plan.target)?;
                return Ok(plan.target);
            }
            Err(e)=>{set(&c,"lastError",&e.to_string())?;log::error!("[storage] migration failed; original data preserved: {e}");return Ok(initial);}
        }
    }reset_versions_selection(&c,&initial)?;Ok(initial)
}
fn reset_versions_selection(locator:&rusqlite::Connection,root:&Path)->Result<(),KernelError>{
    if get(locator,"resetVersionsSelection")?.is_none(){return Ok(());}
    let file=root.join("data/copper.db");if file.is_file(){let conn=rusqlite::Connection::open(file)?;conn.execute("DELETE FROM core_settings WHERE key='game.versions_selection'",[])?;}
    locator.execute("DELETE FROM locator WHERE key='resetVersionsSelection'",[])?;Ok(())
}
pub fn game_override(locator:&Path)->Result<Option<PathBuf>,KernelError>{Ok(get(&db(locator)?,"gameRoot")?.map(PathBuf::from))}
fn execute(plan:&Plan)->Result<Vec<PathBuf>,KernelError>{
    let token=serde_json::to_string(plan)?;
    // A marker is written in staging before activation. After a crash, only this exact
    // transaction's owned destinations may be rolled back; unrelated data is never removed.
    for target in std::iter::once(&plan.target).filter(|_|plan.source!=plan.target).chain(plan.game_root.iter()){
        reject_links(target)?;
        if std::fs::read_to_string(target.join(".cgl-migration-owner")).ok().as_deref()==Some(token.as_str()) {std::fs::remove_dir_all(target)?;}
    }
    let source_db=plan.source.join("data/copper.db");
    let _source_guard=if source_db.is_file(){let conn=rusqlite::Connection::open(source_db)?;conn.busy_timeout(std::time::Duration::from_secs(2))?;let busy:i64=conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)",[],|r|r.get(0))?;if busy!=0{return Err(error("数据库仍被其它启动器进程占用，迁移已停止"));}conn.execute_batch("BEGIN EXCLUSIVE")?;Some(conn)}else{None};
    let mut created=Vec::new();
    let stage_name=format!(".cgl-migration-{:x}",Sha256::digest(token.as_bytes()));
    let stage_for=|target:&Path|->Result<PathBuf,KernelError>{
        let stage=target.with_file_name(format!("{}-{:x}",stage_name,Sha256::digest(target.to_string_lossy().as_bytes())));
        reject_links(&stage)?;
        if stage.exists(){if std::fs::read_to_string(stage.join(".cgl-migration-owner")).ok().as_deref()!=Some(token.as_str()){return Err(error("迁移暂存目录包含未知数据，拒绝覆盖"));}std::fs::remove_dir_all(&stage)?;}
        std::fs::create_dir_all(&stage)?;write_marker(&stage,&token)?;Ok(stage)
    };
    let result=(||{
        separate(&plan.source,&plan.target)?;
        let move_launcher=plan.source!=plan.target;
        if move_launcher{empty_destination(&plan.target)?;}
        if let Some(game)=&plan.game_root{separate(&plan.old_versions,game)?;empty_destination(game)?;}
        // Stage each destination on its own volume, then rename only after complete verification.
        let mut stages:Vec<(PathBuf,PathBuf)>=Vec::new();
        if move_launcher{let stage=stage_for(&plan.target)?;created.push(stage.clone());copy_tree(&plan.source,&stage)?;stages.push((stage,plan.target.clone()));}
        if let Some(game)=&plan.game_root{
            let stage=stage_for(game)?;created.push(stage.clone());std::fs::create_dir_all(stage.join("versions"))?;
            if plan.old_versions.exists(){copy_tree(&plan.old_versions,&stage.join("versions"))?;}
            // Historical packages lived inside versions/.download. Preserve and relocate them.
            let copied_old=stage.join("versions/.download");
            if copied_old.exists(){std::fs::rename(copied_old,stage.join(".download"))?;}
            else if plan.old_packages.exists(){copy_tree(&plan.old_packages,&stage.join(".download"))?;}
            else{std::fs::create_dir_all(stage.join(".download"))?;}
            stages.push((stage,game.clone()));
        }
        // Game-only moves commit their override in the locator transaction after activation.
        if let Some(game)=plan.game_root.as_ref().filter(|_|move_launcher){
            let launcher_stage=&stages[0].0;
            let db_path=launcher_stage.join("data/copper.db");
            let mut conn=rusqlite::Connection::open(db_path)?;let tx=conn.transaction()?;
            for (key,val) in [("game.root",game.clone()),("game.directory",game.join("versions")),("game.versions_selection",PathBuf::new())]{tx.execute("INSERT INTO core_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",rusqlite::params![key,serde_json::to_string(&val.to_string_lossy())?])?;}tx.commit()?;
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        }
        for (stage,target) in stages{write_marker(&stage,&token)?;std::fs::remove_dir(&target)?;std::fs::rename(&stage,&target)?;created.retain(|p|p!=&stage);created.push(target);}
        Ok(())
    })();
    if let Err(e)=result{for p in created.iter().rev(){let _=std::fs::remove_dir_all(p);}return Err(e);}Ok(created)
}
pub fn status(paths:&Paths,settings:&super::settings::SettingsService)->Result<serde_json::Value,KernelError>{
    let c=db(paths.locator_dir())?;
    Ok(serde_json::json!({"launcherRoot":paths.launcher_root(),"gameRoot":paths.game_root(settings),"versions":paths.versions_root(settings),"packages":paths.package_root(settings),"environmentOverride":std::env::var_os("COPPER_DATA_DIR").is_some(),"pending":get(&c,"pending")?.map(|v|serde_json::from_str::<serde_json::Value>(&v)).transpose()?,"lastError":get(&c,"lastError")?,"safSupported":false,"restartRequired":get(&c,"pending")?.is_some()}))
}
pub fn preview(paths:&Paths,settings:&super::settings::SettingsService,request:&StorageRequest)->Result<serde_json::Value,KernelError>{
    let p=plan(paths,settings,request)?;
    Ok(serde_json::json!({"source":p.source,"launcherRoot":p.target,"gameRoot":p.game_root,"restartRequired":true,"preservesSource":true,"requiresLauncherRelocationForGame":false}))
}
fn plan(paths:&Paths,settings:&super::settings::SettingsService,r:&StorageRequest)->Result<Plan,KernelError>{
    let target=r.launcher_root.as_deref().map(path).transpose()?.unwrap_or_else(||paths.launcher_root().clone());
    let game=r.game_root.as_deref().map(path).transpose()?;
    if std::env::var_os("COPPER_DATA_DIR").is_some(){return Err(error("COPPER_DATA_DIR 覆盖生效，请先退出并清除此覆盖后修改持久存储位置"));}
    if target==*paths.launcher_root() && game.is_none(){return Err(error("请选择新的存储位置"));}
    if target!=*paths.launcher_root(){separate(paths.launcher_root(),&target)?;empty_destination(&target)?;}
    if let Some(game)=&game{separate(paths.launcher_root(),game)?;separate(&target,game)?;separate(&paths.versions_root(settings),game)?;empty_destination(game)?;}
    Ok(Plan{source:paths.launcher_root().clone(),target,old_versions:paths.versions_root(settings),old_packages:paths.package_root(settings),game_root:game})
}
pub fn apply(paths:&Paths,settings:&super::settings::SettingsService,r:&StorageRequest)->Result<serde_json::Value,KernelError>{
    let p=plan(paths,settings,r)?;let mut c=db(paths.locator_dir())?;
    let tx=c.transaction()?;if get(&tx,"pending")?.is_some(){return Err(error("已有待执行迁移，请重启完成或取消后重试"));}set(&tx,"pending",&serde_json::to_string(&p)?)?;tx.commit()?;
    Ok(serde_json::json!({"restartRequired":true,"scheduled":true,"completed":false}))
}
pub fn cancel(paths:&Paths)->Result<(),KernelError>{let c=db(paths.locator_dir())?;c.execute("DELETE FROM locator WHERE key IN ('pending','lastError')",[])?;Ok(())}
#[cfg(test)]
mod tests{
 use super::*;
 fn root()->PathBuf{std::env::temp_dir().join(format!("storage-test-{}",uuid::Uuid::new_v4()))}
 #[test] fn bootstrap_keeps_legacy_and_env_wins(){let r=root();let legacy=r.join("legacy");std::fs::create_dir_all(legacy.join("data")).unwrap();std::fs::write(legacy.join("data/copper.db"),b"legacy").unwrap();assert_eq!(bootstrap(&r.join("locator"),&r.join("default"),&legacy,None).unwrap(),legacy);let env=r.join("env");assert_eq!(bootstrap(&r.join("locator"),&r.join("default"),&legacy,Some(env.clone())).unwrap(),env);std::fs::remove_dir_all(r).unwrap();}
 #[test] fn failed_migration_preserves_source_and_locator(){let r=root();let source=r.join("source");std::fs::create_dir_all(&source).unwrap();std::fs::write(source.join("keep"),b"original").unwrap();let target=r.join("target");std::fs::create_dir_all(&target).unwrap();std::fs::write(target.join("occupied"),b"existing").unwrap();let c=db(&r.join("locator")).unwrap();let plan=Plan{source:source.clone(),target:target.clone(),old_versions:source.join("versions"),old_packages:source.join("packages"),game_root:None};set(&c,"pending",&serde_json::to_string(&plan).unwrap()).unwrap();drop(c);assert_eq!(bootstrap(&r.join("locator"),&source,&r.join("legacy"),None).unwrap(),source);assert_eq!(std::fs::read(source.join("keep")).unwrap(),b"original");assert!(target.join("occupied").exists());std::fs::remove_dir_all(r).unwrap();}
 #[test] fn successful_bootstrap_copies_and_restarts_at_target(){let r=root();let source=r.join("source");std::fs::create_dir_all(&source).unwrap();std::fs::write(source.join("keep"),b"original").unwrap();let target=r.join("target");let c=db(&r.join("locator")).unwrap();let p=Plan{source:source.clone(),target:target.clone(),old_versions:source.join("versions"),old_packages:source.join("packages"),game_root:None};set(&c,"pending",&serde_json::to_string(&p).unwrap()).unwrap();drop(c);assert_eq!(bootstrap(&r.join("locator"),&source,&r.join("legacy"),None).unwrap(),target);assert!(source.join("keep").exists());assert!(target.join("keep").exists());assert_eq!(bootstrap(&r.join("locator"),&source,&r.join("legacy"),None).unwrap(),target);std::fs::remove_dir_all(r).unwrap();}
 #[test] fn game_only_restart_moves_versions_and_packages(){let r=root();let source=r.join("source");let old=source.join("data/versions");std::fs::create_dir_all(old.join(".download")).unwrap();std::fs::write(old.join(".download/package.msix"),b"package").unwrap();std::fs::create_dir_all(old.join("1.0")).unwrap();std::fs::write(old.join("1.0/version.json"),b"{}").unwrap();let game=r.join("game");let locator=r.join("locator");let c=db(&locator).unwrap();let p=Plan{source:source.clone(),target:source.clone(),old_versions:old.clone(),old_packages:old.join(".download"),game_root:Some(game.clone())};set(&c,"pending",&serde_json::to_string(&p).unwrap()).unwrap();drop(c);assert_eq!(bootstrap(&locator,&source,&r.join("legacy"),None).unwrap(),source);assert_eq!(game_override(&locator).unwrap(),Some(game.clone()));assert!(game.join("versions/1.0/version.json").exists());assert!(game.join(".download/package.msix").exists());assert!(old.join(".download/package.msix").exists());std::fs::remove_dir_all(r).unwrap();}
 #[test] fn interrupted_activation_is_recovered_only_with_matching_marker(){let r=root();let source=r.join("source");std::fs::create_dir_all(&source).unwrap();std::fs::write(source.join("keep"),b"original").unwrap();let target=r.join("target");let locator=r.join("locator");let p=Plan{source:source.clone(),target:target.clone(),old_versions:source.join("versions"),old_packages:source.join("packages"),game_root:None};let c=db(&locator).unwrap();set(&c,"pending",&serde_json::to_string(&p).unwrap()).unwrap();drop(c);execute(&p).unwrap();assert!(target.join(".cgl-migration-owner").exists());assert_eq!(bootstrap(&locator,&source,&r.join("legacy"),None).unwrap(),target);assert!(target.join("keep").exists());assert!(!target.join(".cgl-migration-owner").exists());assert!(source.join("keep").exists());std::fs::remove_dir_all(r).unwrap();}
 #[test] fn saf_and_relative_paths_are_rejected(){assert!(path("content://documents/tree/1").is_err());assert!(path("relative/folder").is_err());}
}
