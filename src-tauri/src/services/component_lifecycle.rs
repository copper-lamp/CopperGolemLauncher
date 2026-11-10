//! 组件版本读租约与受限恢复/回收。读租约跨进程持有；清理必须取得独占句柄。
use std::{fs::{self,File,OpenOptions},path::Path,time::{Duration,SystemTime}};
use crate::error::KernelError;
pub fn lease(version:&Path)->Result<File,KernelError>{
    let mut options=OpenOptions::new();options.read(true);
    #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt;options.share_mode(1);}
    Ok(options.open(version.join(".lease"))?)
}
pub fn create_lease(version:&Path)->Result<(),KernelError>{OpenOptions::new().write(true).create_new(true).open(version.join(".lease"))?.sync_all()?;Ok(())}
fn old(path:&Path,age:Duration)->bool{fs::symlink_metadata(path).ok().filter(|m|m.is_dir()&&!m.file_type().is_symlink()).and_then(|m|m.modified().ok()).and_then(|t|SystemTime::now().duration_since(t).ok()).is_some_and(|a|a>=age)}
/// 调用者必须已持有该组件 install.lock。仅服务生成的UUID staging可删除。
pub fn recover_staging(base:&Path){
    let Ok(entries)=fs::read_dir(base)else{return;};
    for entry in entries.flatten(){let name=entry.file_name();let Some(name)=name.to_str()else{continue;};if name.strip_prefix("staging-").is_some_and(|s|uuid::Uuid::parse_str(s).is_ok())&&old(&entry.path(),Duration::from_secs(6*3600)){let _=fs::remove_dir_all(entry.path());}}
}
/// 留存当前和上次激活版本，其余至少保留7天。Windows独占lease失败即正在使用。
pub fn cleanup_versions(base:&Path,keep:&[String]){
    if !cfg!(windows){return;} // 其他平台还没有跨进程共享文件锁，禁止冒险回收。
    let Ok(entries)=fs::read_dir(base.join("versions"))else{return;};
    for entry in entries.flatten(){let name=entry.file_name().to_string_lossy().into_owned();if keep.contains(&name)||!old(&entry.path(),Duration::from_secs(7*86400)){continue;}
        // 目录名结尾必须是服务产生的规范UUID，不接受任意外部目录。
        if name.len()<37||!name.get(name.len()-36..).is_some_and(|s|uuid::Uuid::parse_str(s).is_ok()){continue;}
        let mut options=OpenOptions::new();options.read(true).write(true);
        #[cfg(windows)]{use std::os::windows::fs::OpenOptionsExt;options.share_mode(4);}
        if let Ok(guard)=options.open(entry.path().join(".lease")){
            // 先原子移出versions命名空间；Windows仍持有独占lease，读者无法新建租约。
            let retired=base.join(format!("staging-{}",uuid::Uuid::new_v4()));
            // Windows目录重命名不改变已打开lease文件身份；若系统拒绝则保守保留。
            if fs::rename(entry.path(),&retired).is_ok(){drop(guard);let _=fs::remove_dir_all(retired);}
        }
    }
}
#[cfg(test)]mod tests{use super::*;
#[test]fn invalid_staging_is_never_removed(){let root=std::env::temp_dir().join(format!("component-recovery-{}",uuid::Uuid::new_v4()));fs::create_dir_all(root.join("staging-user-files")).unwrap();recover_staging(&root);assert!(root.join("staging-user-files").is_dir());let _=fs::remove_dir_all(root);}
#[cfg(windows)]#[test]fn lease_denies_exclusive_cleanup_until_reader_drops(){use std::os::windows::fs::OpenOptionsExt;let root=std::env::temp_dir().join(format!("component-lease-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();create_lease(&root).unwrap();let reader=lease(&root).unwrap();assert!(OpenOptions::new().read(true).write(true).share_mode(0).open(root.join(".lease")).is_err());drop(reader);assert!(OpenOptions::new().read(true).write(true).share_mode(0).open(root.join(".lease")).is_ok());let _=fs::remove_dir_all(root);}}
