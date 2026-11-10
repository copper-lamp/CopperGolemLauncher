//! 内核受管外部组件：官方元数据、完整性校验、不可变版本与原子激活。
//! lip 保持 0.37 daemon 协议；新主协议只能随启动器适配后扩展。
use std::{collections::HashMap, fs::{self, File, OpenOptions}, io::{Read, Write}, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Duration};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use crate::{error::KernelError, registry::events::EventBus};
use super::{paths::Paths, settings::SettingsService};
#[path="component_lifecycle.rs"] pub mod lifecycle;
use std::sync::atomic::{AtomicU64,Ordering};
#[derive(Default)] struct JobControl { generation:AtomicU64, foreground:AtomicU64 }
struct Foreground<'a>(&'a AtomicU64);
impl Drop for Foreground<'_>{fn drop(&mut self){self.0.fetch_sub(1,Ordering::SeqCst);}}

const DOTNET_METADATA: &str = "https://builds.dotnet.microsoft.com/dotnet/release-metadata/10.0/releases.json";
const LIP_RELEASES: &str = "https://api.github.com/repos/LiteLDev/lip/releases?per_page=100";
const MAX_ARCHIVE: usize = 150 * 1024 * 1024;
const MAX_EXPANDED: u64 = 600 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentId { Lip, Dotnet10 }
impl ComponentId {
    pub fn key(self) -> &'static str { match self { Self::Lip => "lip", Self::Dotnet10 => "dotnet10" } }
    fn executable(self) -> &'static str { match self { Self::Lip => "lipd.exe", Self::Dotnet10 => "dotnet.exe" } }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStatus { pub id: ComponentId, pub state: String, pub version: Option<String>, pub available_version: Option<String>, pub stage: Option<String>, pub error: Option<String>, pub supported: bool }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Active { version: String, rid: String, directory: String, executable_sha256: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Asset { version: String, rid: String, url: String, hash: String, algorithm: String }
/// 持有不可变版本路径即版本租约；服务不会覆盖/删除历史版本或运行中的文件。
#[derive(Clone)]
pub struct LipRuntime { pub exe: PathBuf, pub dotnet_root: PathBuf, pub user_home: PathBuf, _leases:Arc<Vec<File>> }
pub struct ExternalComponentService {
    root: PathBuf, settings: Arc<SettingsService>, events: Arc<EventBus>,
    gates: HashMap<ComponentId, tokio::sync::Mutex<()>>,
    states: Mutex<HashMap<ComponentId, ComponentStatus>>, client: reqwest::Client, control:JobControl,
}
impl ExternalComponentService {
    pub fn new(paths: &Paths, settings: Arc<SettingsService>, events: Arc<EventBus>) -> Self {
        Self { root: paths.data_dir().join("components"), settings, events,
            gates: [(ComponentId::Lip, tokio::sync::Mutex::new(())), (ComponentId::Dotnet10, tokio::sync::Mutex::new(()))].into(),
            states: Mutex::new(HashMap::new()), control:JobControl::default(), client: super::http_client::client_builder(Duration::from_secs(180)).user_agent("CopperGolem-components").redirect(reqwest::redirect::Policy::custom(|attempt| { if attempt.previous().len()>=8 || !download_host(attempt.url()) {attempt.error("component_redirect_rejected")} else {attempt.follow()} })).build().expect("component HTTP client") }
    }
    pub fn supported() -> bool { cfg!(windows) && matches!(std::env::consts::ARCH, "x86_64" | "aarch64") }
    fn rid() -> &'static str { if cfg!(target_arch="aarch64") { "win-arm64" } else { "win-x64" } }
    fn component_dir(&self, id: ComponentId) -> PathBuf { self.root.join(id.key()) }
    fn active(&self, id: ComponentId) -> Result<(Active, PathBuf), KernelError> {
        let active: Active = serde_json::from_slice(&fs::read(self.component_dir(id).join("current.json"))?)?;
        if active.rid != Self::rid() || !safe_segment(&active.directory) || !compatible(id, &active.version) { return Err(failure("component_manifest_invalid")); }
        let dir = self.component_dir(id).join("versions").join(&active.directory);
        if sha256_file(&dir.join(id.executable()))? != active.executable_sha256 { return Err(failure("component_hash_mismatch")); }
        if !dir.join(".lease").is_file(){match lifecycle::create_lease(&dir){Ok(())=>{},Err(_) if dir.join(".lease").is_file()=>{},Err(e)=>return Err(e)}}
        Ok((active, dir))
    }
    pub fn installed_path(&self, id: ComponentId) -> Option<PathBuf> { self.active(id).ok().map(|(_,p)| p.join(id.executable())) }
    pub fn snapshot(&self) -> Vec<ComponentStatus> {
        [ComponentId::Dotnet10, ComponentId::Lip].into_iter().map(|id| {
            let installed = self.active(id).ok();
            let saved = self.states.lock().unwrap().get(&id).cloned();
            ComponentStatus { id, state: saved.as_ref().map(|s|s.state.clone()).unwrap_or_else(|| if !Self::supported() { "unsupported" } else if installed.is_some() { "installed" } else { "missing" }.into()),
                version: installed.map(|(a,_)|a.version), available_version: saved.as_ref().and_then(|s|s.available_version.clone()), stage: saved.as_ref().and_then(|s|s.stage.clone()), error: saved.and_then(|s|s.error), supported: Self::supported() }
        }).collect()
    }
    fn report(&self, id: ComponentId, state: &str, stage: Option<&str>, available: Option<String>, error: Option<String>) {
        let version = self.active(id).ok().map(|(a,_)|a.version);
        let status = ComponentStatus { id, state:state.into(), version, available_version:available, stage:stage.map(str::to_owned), error, supported:Self::supported() };
        self.states.lock().unwrap().insert(id, status.clone());
        self.events.publish("components.changed", serde_json::to_value(status).unwrap_or(Value::Null));
    }
    pub async fn probe(&self, id: ComponentId) -> Result<(), KernelError> {
        if !Self::supported() { return Err(failure("component_platform_unsupported")); }
        let (active, dir) = self.active(id)?;
        self.health(id, &dir, &active.version).await
    }
    pub async fn lip_runtime(&self) -> Result<LipRuntime, KernelError> {
        self.probe(ComponentId::Dotnet10).await?;
        self.probe(ComponentId::Lip).await?;
        let lip=self.active(ComponentId::Lip)?.1;let dotnet=self.active(ComponentId::Dotnet10)?.1;
        let leases=Arc::new(vec![lifecycle::lease(&lip)?,lifecycle::lease(&dotnet)?]);
        Ok(LipRuntime { exe:lip.join("lipd.exe"), dotnet_root:dotnet, user_home:self.root.join("lip-user"),_leases:leases })
    }
    pub async fn check(&self, id: ComponentId) -> Result<ComponentStatus, KernelError> {
        if !Self::supported() { return Err(failure("component_platform_unsupported")); }
        let asset = self.resolve(id).await?;
        let current = self.active(id).ok().map(|(a,_)|a.version);
        let newer = current.as_deref().and_then(|v|semver::Version::parse(v).ok()).map(|v| semver::Version::parse(&asset.version).map(|a|a>v).unwrap_or(false)).unwrap_or(true);
        self.report(id, if newer { "update-available" } else { "installed" }, None, Some(asset.version), None);
        Ok(self.snapshot().into_iter().find(|s|s.id==id).unwrap())
    }
    pub fn cancel(&self){self.control.generation.fetch_add(1,Ordering::SeqCst);}
    async fn controlled<T>(&self,generation:u64,future:impl std::future::Future<Output=Result<T,KernelError>>)->Result<T,KernelError>{
        tokio::pin!(future);
        loop{tokio::select!{result=&mut future=>return result,_=tokio::time::sleep(Duration::from_millis(100))=>{if self.control.generation.load(Ordering::SeqCst)!=generation{return Err(failure("component_cancelled"));}}}}
    }
    pub async fn install(&self, id: ComponentId) -> Result<(), KernelError> {
        self.control.foreground.fetch_add(1,Ordering::SeqCst);let _foreground=Foreground(&self.control.foreground);
        self.cancel(); // 前台请求取消低优先级网络/健康检查，组件临界区仍串行。
        if !Self::supported() { return Err(failure("component_platform_unsupported")); }
        // 顺序固定；两个并发调用在组件锁内重新判断版本，合并重复安装。
        if id == ComponentId::Lip { self.install_one(ComponentId::Dotnet10).await?; }
        self.install_one(id).await
    }
    async fn install_one(&self, id: ComponentId) -> Result<(), KernelError> {
        let _gate = self.gates[&id].lock().await;
        fs::create_dir_all(self.component_dir(id))?;
        let _process_lock = lock_component(&self.component_dir(id).join("install.lock"))?;
        self.report(id, "installing", Some("metadata"), None, None);
        lifecycle::recover_staging(&self.component_dir(id));
        let result = self.install_locked(id).await;
        match &result { Ok(()) => self.report(id,"installed",None,None,None), Err(e) => self.report(id,"failed",None,None,Some(e.to_string())) }
        result
    }
    async fn install_locked(&self, id: ComponentId) -> Result<(), KernelError> {
        let generation=self.control.generation.load(Ordering::SeqCst);
        let asset = self.controlled(generation,self.resolve(id)).await?;
        if let Ok((active,dir)) = self.active(id) {
            let old = semver::Version::parse(&active.version).map_err(|_|failure("component_version_invalid"))?;
            let new = semver::Version::parse(&asset.version).map_err(|_|failure("component_version_invalid"))?;
            if old >= new && self.health(id,&dir,&active.version).await.is_ok() { return Ok(()); }
        }
        let base = self.component_dir(id);
        let stage = base.join(format!("staging-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&stage)?;
        let cleanup = Staging(stage.clone());
        self.report(id,"installing",Some("downloading"),Some(asset.version.clone()),None);
        let bytes = self.controlled(generation,self.fetch_bytes(&asset.url, MAX_ARCHIVE)).await?;
        verify_hash(&bytes,&asset.hash,&asset.algorithm)?;
        self.report(id,"installing",Some("extracting"),Some(asset.version.clone()),None);
        let unpack = stage.clone();
        tokio::task::spawn_blocking(move || extract_zip(&bytes,&unpack)).await.map_err(|_|failure("component_extract_join"))??;
        lifecycle::create_lease(&stage)?;
        self.report(id,"installing",Some("health-check"),Some(asset.version.clone()),None);
        if self.control.generation.load(Ordering::SeqCst)!=generation{return Err(failure("component_cancelled"));}
        self.controlled(generation,self.health(id,&stage,&asset.version)).await?;
        // 唯一不可变版本目录避免维修同版本时覆写现用 exe。
        let directory = format!("{}-{}-{}",asset.version,asset.rid,uuid::Uuid::new_v4());
        let versions = base.join("versions"); fs::create_dir_all(&versions)?;
        let destination = versions.join(&directory);
        let executable_sha256 = sha256_file(&stage.join(id.executable()))?;
        fs::rename(&stage,&destination)?;
        drop(cleanup);
        let previous=self.active(id).ok().map(|(a,_)|a.directory);
        let active = Active { version:asset.version, rid:asset.rid, directory, executable_sha256 };
        // 原子替换一个很小的指针。失败/崩溃时旧指针和旧版本保持可用。
        atomic_json(&base.join("current.json"),&active)?;
        let mut keep=vec![active.directory.clone()];if let Some(previous)=previous{keep.push(previous);}
        lifecycle::cleanup_versions(&base,&keep);
        Ok(())
    }
    async fn health(&self, id:ComponentId, dir:&Path, version:&str) -> Result<(),KernelError> {
        let exe=dir.join(id.executable());
        let mut command=tokio::process::Command::new(&exe);
        command.kill_on_drop(true);
        if id==ComponentId::Dotnet10 { command.arg("--list-runtimes"); }
        else {
            let runtime=self.active(ComponentId::Dotnet10)?.1;
            let home=self.root.join("lip-user"); fs::create_dir_all(&home)?;
            command.arg("--version").env("DOTNET_ROOT",&runtime).env("DOTNET_ROOT_X64",&runtime).env("DOTNET_ROOT_ARM64",&runtime).env("DOTNET_MULTILEVEL_LOOKUP","0").env("APPDATA",&home).env("XDG_CONFIG_HOME",&home);
        }
        #[cfg(windows)] command.creation_flags(0x08000000);
        let out=tokio::time::timeout(Duration::from_secs(12),command.output()).await.map_err(|_|failure("component_health_timeout"))??;
        let text=String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || (id==ComponentId::Dotnet10 && !text.lines().any(|l|l.starts_with(&format!("Microsoft.NETCore.App {version} ")))) || (id==ComponentId::Lip && !text.contains(version)) { return Err(failure("component_health_failed")); }
        if id==ComponentId::Lip {
            let workspace=self.root.join("health-workspace");fs::create_dir_all(&workspace)?;
            let result=tokio::time::timeout(Duration::from_secs(15),crate::modules::content_download::lipd::call(&exe,&workspace,"List",serde_json::json!([]))).await;
            match result { Ok(Ok(_))=>{},_=>return Err(failure("component_rpc_health_failed")) }
        }
        Ok(())
    }
    async fn resolve(&self,id:ComponentId)->Result<Asset,KernelError> {
        let url=if id==ComponentId::Lip { LIP_RELEASES } else { DOTNET_METADATA };
        let bytes=self.fetch_bytes(url,8*1024*1024).await?;
        let data:Value=serde_json::from_slice(&bytes)?;
        let rid=Self::rid();
        let asset=if id==ComponentId::Lip { resolve_lip(&data,rid)? } else { resolve_dotnet(&data,rid)? };
        fs::create_dir_all(&self.root)?;
        atomic_json(&self.root.join(format!("{}-metadata.json",id.key())),&asset)?;
        Ok(asset)
    }
    async fn fetch_bytes(&self,url:&str,limit:usize)->Result<Vec<u8>,KernelError> {
        if !official_url(url) { return Err(failure("component_source_rejected")); }
        let mut response=self.client.get(url).send().await?.error_for_status()?;
        if response.url().scheme()!="https" {return Err(failure("component_redirect_rejected"));}
        if response.content_length().is_some_and(|n|n>limit as u64) {return Err(failure("component_download_too_large"));}
        let mut bytes=Vec::new();
        while let Some(chunk)=response.chunk().await? { if bytes.len()+chunk.len()>limit {return Err(failure("component_download_too_large"));} bytes.extend_from_slice(&chunk); }
        Ok(bytes)
    }
    pub fn spawn_background(self:&Arc<Self>, runtime:&tokio::runtime::Handle) {
        // 调度对象待用户明确；独立安全批不启动周期网络，不检查任何已安装组件。
        // 完整待定调度方案保存在 docs/铜核心/外部组件后台调度待定.md。
        let _ = (runtime,&self.settings);
    }
}
fn failure(code:&str)->KernelError {KernelError::Module(code.into())}
fn safe_segment(s:&str)->bool {!s.is_empty() && s.chars().all(|c|c.is_ascii_alphanumeric() || c=='-' || c=='.') && s!="." && s!=".."}
fn compatible(id:ComponentId,v:&str)->bool { semver::Version::parse(v).is_ok_and(|v| v.pre.is_empty() && match id {ComponentId::Lip=>v.major==0&&v.minor==37,ComponentId::Dotnet10=>v.major==10}) }
fn download_host(u:&reqwest::Url)->bool { u.scheme()=="https" && matches!(u.host_str(),Some("github.com"|"api.github.com"|"builds.dotnet.microsoft.com"|"download.visualstudio.microsoft.com"|"release-assets.githubusercontent.com"|"objects.githubusercontent.com")) }
fn official_url(raw:&str)->bool { reqwest::Url::parse(raw).is_ok_and(|u|u.scheme()=="https" && matches!(u.host_str(),Some("github.com"|"api.github.com"|"builds.dotnet.microsoft.com"|"download.visualstudio.microsoft.com"|"ci.dot.net"))) }
fn resolve_lip(data:&Value,rid:&str)->Result<Asset,KernelError> {
    let releases=data.as_array().ok_or_else(||failure("component_metadata_invalid"))?;
    let mut candidates=Vec::new();
    for r in releases {
        let Some(version)=r["tag_name"].as_str().map(|s|s.trim_start_matches('v')) else {continue};
        if r["draft"]==true || r["prerelease"]==true || !compatible(ComponentId::Lip,version) {continue;}
        let name=format!("lip-{version}-{rid}.zip");
        for a in r["assets"].as_array().into_iter().flatten() {
            if a["name"].as_str()!=Some(&name) {continue;}
            let Some(hash)=a["digest"].as_str().and_then(|s|s.strip_prefix("sha256:")) else {continue};
            let Some(url)=a["browser_download_url"].as_str() else {continue};
            if official_url(url) {candidates.push(Asset{version:version.into(),rid:rid.into(),url:url.into(),hash:hash.into(),algorithm:"sha256".into()});}
        }
    }
    candidates.sort_by_key(|a|semver::Version::parse(&a.version).ok());
    candidates.pop().ok_or_else(||failure("component_compatible_release_missing"))
}
fn resolve_dotnet(data:&Value,rid:&str)->Result<Asset,KernelError> {
    let version=data["latest-runtime"].as_str().filter(|v|compatible(ComponentId::Dotnet10,v)).ok_or_else(||failure("component_metadata_invalid"))?;
    let file=data["releases"].as_array().into_iter().flatten().filter(|r|r["runtime"]["version"]==version).flat_map(|r|r["runtime"]["files"].as_array().into_iter().flatten()).find(|f|f["rid"]==rid&&f["name"].as_str()==Some(&format!("dotnet-runtime-{rid}.zip"))).ok_or_else(||failure("component_asset_missing"))?;
    let url=file["url"].as_str().filter(|s|official_url(s)).ok_or_else(||failure("component_source_rejected"))?;
    Ok(Asset{version:version.into(),rid:rid.into(),url:url.into(),hash:file["hash"].as_str().ok_or_else(||failure("component_hash_missing"))?.into(),algorithm:"sha512".into()})
}
fn verify_hash(bytes:&[u8],expected:&str,algorithm:&str)->Result<(),KernelError> {
    let actual=match algorithm {"sha256"=>format!("{:x}",Sha256::digest(bytes)),"sha512"=>format!("{:x}",Sha512::digest(bytes)),_=>return Err(failure("component_hash_algorithm"))};
    if actual!=expected.to_ascii_lowercase(){return Err(failure("component_hash_mismatch"));} Ok(())
}
fn sha256_file(path:&Path)->Result<String,KernelError> {let mut f=File::open(path)?;let mut h=Sha256::new();let mut b=[0u8;65536];loop{let n=f.read(&mut b)?;if n==0{break;}h.update(&b[..n]);}Ok(format!("{:x}",h.finalize()))}
fn extract_zip(bytes:&[u8],dir:&Path)->Result<(),KernelError>{
    let mut archive=zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_|failure("component_archive_invalid"))?;
    if archive.len()>10000{return Err(failure("component_archive_too_large"));}
    let mut size=0u64;
    for i in 0..archive.len(){let mut entry=archive.by_index(i).map_err(|_|failure("component_archive_invalid"))?;
        size=size.checked_add(entry.size()).ok_or_else(||failure("component_archive_too_large"))?;if size>MAX_EXPANDED{return Err(failure("component_archive_too_large"));}
        let relative=entry.enclosed_name().ok_or_else(||failure("component_archive_path"))?.to_path_buf();
        if entry.name().contains('\\') || relative.components().any(|p|p.as_os_str().to_string_lossy().contains(':')) || entry.unix_mode().is_some_and(|m|m&0o170000==0o120000){return Err(failure("component_archive_path"));}
        let output=dir.join(relative);if entry.is_dir(){fs::create_dir_all(&output)?;continue;}
        if let Some(parent)=output.parent(){fs::create_dir_all(parent)?;}
        let mut f=OpenOptions::new().create_new(true).write(true).open(output)?;std::io::copy(&mut entry,&mut f)?;f.sync_all()?;
    }Ok(())
}
fn lock_component(path:&Path)->Result<File,KernelError>{
    let mut options=OpenOptions::new();options.create(true).read(true).write(true);
    #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt;options.share_mode(0);}
    Ok(options.open(path).map_err(|_|failure("component_busy"))?)
}
fn atomic_json(path:&Path,value:&impl Serialize)->Result<(),KernelError>{
    let tmp=path.with_extension(format!("{}.tmp",uuid::Uuid::new_v4()));
    let mut file=OpenOptions::new().create_new(true).write(true).open(&tmp)?;
    file.write_all(&serde_json::to_vec(value)?)?;file.sync_all()?;drop(file);
    #[cfg(windows)] {use windows::{core::PCWSTR,Win32::Storage::FileSystem::{MoveFileExW,MOVEFILE_REPLACE_EXISTING,MOVEFILE_WRITE_THROUGH}};use std::os::windows::ffi::OsStrExt;
        let a:Vec<u16>=tmp.as_os_str().encode_wide().chain(Some(0)).collect();let b:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe{MoveFileExW(PCWSTR(a.as_ptr()),PCWSTR(b.as_ptr()),MOVEFILE_REPLACE_EXISTING|MOVEFILE_WRITE_THROUGH)}.map_err(|_|failure("component_activate_failed"))?;
    }
    #[cfg(not(windows))] fs::rename(&tmp,path)?;
    Ok(())
}
struct Staging(PathBuf);impl Drop for Staging{fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    /// 手动运行：真实官方资产，所有current/配置均仅位于本测试独立临时树。
    #[tokio::test] #[ignore = "downloads official assets and executes isolated health checks"]
    async fn official_assets_health_in_isolated_directory(){
        if !ExternalComponentService::supported(){return;}
        let root=std::env::temp_dir().join(format!("copper-official-validation-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();let _cleanup=Staging(root.clone());
        let paths=Paths::with_root(root.clone());fs::create_dir_all(paths.data_dir()).unwrap();
        let db=Arc::new(super::super::database::DatabaseService::open(&root.join("validation.sqlite")).unwrap());db.migrate_scope("core",super::super::database::CORE_MIGRATIONS).unwrap();
        let events=Arc::new(EventBus::new());let settings=Arc::new(SettingsService::new(db,events.clone(),Default::default()).unwrap());
        let service=ExternalComponentService::new(&paths,settings,events);
        service.install(ComponentId::Lip).await.unwrap();
        service.probe(ComponentId::Dotnet10).await.unwrap();service.probe(ComponentId::Lip).await.unwrap();
        let runtime=service.lip_runtime().await.unwrap();println!("official isolated health passed: {}",runtime.exe.display());drop(runtime);
    }
    #[test] fn compatibility_never_selects_daemon_removed_release(){
        let data=json!([
          {"tag_name":"v0.40.0","assets":[{"name":"lip-0.40.0-win-x64.zip","digest":format!("sha256:{}","a".repeat(64)),"browser_download_url":"https://github.com/LiteLDev/lip/releases/download/v0.40.0/lip-0.40.0-win-x64.zip"}]},
          {"tag_name":"v0.37.0","assets":[{"name":"lip-0.37.0-win-x64.zip","digest":format!("sha256:{}","b".repeat(64)),"browser_download_url":"https://github.com/LiteLDev/lip/releases/download/v0.37.0/lip-0.37.0-win-x64.zip"}]}
        ]);
        assert_eq!(resolve_lip(&data,"win-x64").unwrap().version,"0.37.0");
        assert!(resolve_lip(&data,"win-arm64").is_err());
        assert!(!compatible(ComponentId::Lip,"0.37.1-beta.1"));
    }
    #[test] fn wrong_digest_and_untrusted_origin_fail_closed(){
        assert!(verify_hash(b"payload",&"0".repeat(64),"sha256").is_err());
        assert!(verify_hash(b"payload",&format!("{:x}",Sha256::digest(b"payload")),"sha256").is_ok());
        assert!(!official_url("http://github.com/LiteLDev/lip"));
        assert!(!official_url("https://github.com.evil.test/lip"));
        assert!(!download_host(&reqwest::Url::parse("https://evil.test/redirect").unwrap()));
    }
    #[test] fn zip_traversal_rejected_without_escaping_staging(){
        let root=std::env::temp_dir().join(format!("copper-component-test-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();let _cleanup=Staging(root.clone());
        let mut writer=zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer.start_file("../escape",zip::write::SimpleFileOptions::default()).unwrap();writer.write_all(b"bad").unwrap();let data=writer.finish().unwrap().into_inner();
        assert!(extract_zip(&data,&root).is_err());assert!(!root.join("escape").exists());
    }
    #[test] fn atomic_pointer_keeps_previous_until_valid_replacement(){
        let root=std::env::temp_dir().join(format!("copper-component-pointer-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();let _cleanup=Staging(root.clone());
        let path=root.join("current.json");atomic_json(&path,&json!({"version":"old"})).unwrap();
        assert!(verify_hash(b"corrupt",&"0".repeat(64),"sha256").is_err());
        assert_eq!(serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["version"],"old");
        atomic_json(&path,&json!({"version":"new"})).unwrap();assert_eq!(serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["version"],"new");
    }
}
