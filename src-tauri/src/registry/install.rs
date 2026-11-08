//! 附加模块安装链路：下载 → 校验 → 解包 → 原子落位 → 一致性复核。
//!
//! 对应 [cgl-libs](../../../docs/cgl-libs.md) 2.9.5「安装流程」：
//!
//! ```text
//! 校验可安装性（平台 / 内核区间 / api_version / yank）
//!   → 投递下载队列（expected_sha256=资产摘要，引擎负责校验）
//!   → 等待下载完成
//!   → 解包到 <modules_dir>/.tmp-<id>（安全校验见 registry::package）
//!   → 包内清单与远端条目 id/version/api_version 一致？（防「元数据说 A、包里是 B」）
//!   → 原子 rename 到 <modules_dir>/<id>
//! ```
//!
//! 安装完成后**不立即装载**（装载只在启动期执行一次，见 [`crate::registry::loader`]），
//! 由调用方提示用户重启——这是产品决策「重启生效」。

use std::time::{Duration, Instant};

use copper_downloader::{DownloadOptions, DownloadStatus};

use crate::error::KernelError;
use crate::registry::modules::is_valid_module_id;
use crate::registry::package;
use crate::services::registry::model::Platform;
use crate::state::KernelContext;

static MUTATING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) struct MutationGuard;
impl Drop for MutationGuard { fn drop(&mut self) { MUTATING.store(false, std::sync::atomic::Ordering::Release); } }
pub(crate) fn begin_mutation() -> Result<MutationGuard, KernelError> {
    MUTATING.compare_exchange(false, true, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire)
        .map_err(|_| KernelError::Module("另一个模块安装或卸载正在进行".into()))?;
    Ok(MutationGuard)
}

/// 等待下载完成的最长时间（防止异常情况下命令永不返回）。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// 轮询下载状态的间隔。
const DOWNLOAD_POLL_INTERVAL: Duration = Duration::from_millis(300);

/// 安装结果（供命令层返回前端）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallOutcome {
    pub id: String,
    pub version: String,
    /// 是否需要重启才能生效（当前恒为 true）。
    pub restart_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_sha256: Option<String>,
}

/// 安装一个远端条目。返回安装结果或明确错误。
pub async fn install_addon(kernel: &KernelContext, id: &str) -> Result<InstallOutcome, KernelError> {
    let _operation = begin_mutation()?;
    let id = id.trim();
    if !is_valid_module_id(id) {
        return Err(KernelError::Module(format!(
            "模块 id `{id}` 不合法：必须为两段式小写形态"
        )));
    }

    if kernel.modules().is_registered(id) && (kernel.modules().origin_of(id).is_builtin() || kernel.modules().is_running(id)) {
        return Err(KernelError::Module("不能替换内置或正在运行的模块，请禁用并重启后重试".into()));
    }
    let registry = kernel
        .registry()
        .ok_or_else(|| KernelError::Config("元数据服务未初始化".into()))?;

    // 确保索引已装载：首次安装前用户可能从未刷新过元数据。
    let _ = registry.load_index(false).await;

    let (entries, _) = registry.list_modules().await?;
    let entry = entries
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| KernelError::Module(format!("元数据中不存在模块 `{id}`")))?;

    // 1) 可安装性（平台 / 内核区间 / api_version / yank / 资产摘要）。
    let platform = Platform::current();
    let asset = platform
        .as_ref()
        .and_then(|p| entry.asset_for(p.clone()).cloned());
    if let Some(reason) = crate::services::registry::install_block_reason(
        &entry,
        asset.as_ref(),
        env!("CARGO_PKG_VERSION"),
    ) {
        return Err(KernelError::Module(format!(
            "模块 `{id}` 当前不可安装：{reason}"
        )));
    }
    let asset = asset.ok_or_else(|| {
        KernelError::Module(format!("模块 `{id}` 无当前平台可用资产"))
    })?;

    // 2) 下载到缓存（sha256 由下载引擎校验，避免业务层重复实现）。
    let assets_dir = kernel.paths().cache_dir().join("registry").join("assets");
    std::fs::create_dir_all(&assets_dir)?;
    let file_name = format!("{}-{}.cglm", id, entry.version);
    let dest = assets_dir.join(&file_name);
    let task_id = kernel.download().enqueue(
        &asset.url,
        &dest,
        DownloadOptions {
            resume: true,
            remove_on_cancel: true,
            expected_sha256: Some(asset.sha256.clone()),
            filename: Some(file_name.clone()),
            ..Default::default()
        },
    )?;
    await_download(kernel, task_id).await?;

    // 3) 解包到临时目录（任一步失败都不落位，保持「要么旧版、要么完整新版」）。
    let modules_dir = kernel.paths().modules_dir().clone();
    let tmp_dir = modules_dir.join(format!(".tmp-{id}"));
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let staged = async {
        let extracted = package::extract_package(&dest, &tmp_dir)?;

        // 4) 包内清单必须与远端条目一致。
        if extracted.manifest.id != entry.id {
            return Err(KernelError::Module(format!(
                "模块包内 id `{}` 与远端条目 `{}` 不一致，拒绝安装",
                extracted.manifest.id, entry.id
            )));
        }
        if extracted.manifest.version != entry.version {
            return Err(KernelError::Module(format!(
                "模块包内版本 `{}` 与远端条目 `{}` 不一致，拒绝安装",
                extracted.manifest.version, entry.version
            )));
        }
        if extracted.manifest.api_version as i64 != entry.api_version {
            return Err(KernelError::Module(format!(
                "模块包内 api_version {} 与远端条目 {} 不一致，拒绝安装",
                extracted.manifest.api_version, entry.api_version
            )));
        }

        // 5) 清单语义 + 平台 / 内核区间再校验（不信任远端元数据之外的包内声明）。
        extracted.manifest.validate()?;
        if !extracted.manifest.supports_current_platform() {
            return Err(KernelError::Module(format!(
                "模块 `{id}` 的包未声明支持当前平台"
            )));
        }
        if !extracted.manifest.accepts_launcher(env!("CARGO_PKG_VERSION")) {
            return Err(KernelError::Module(format!(
                "模块 `{id}` 声明的内核兼容区间不覆盖当前内核 {}",
                env!("CARGO_PKG_VERSION")
            )));
        }

        // 6) 原子落位。
        package::promote_extracted(&tmp_dir, &modules_dir, id)
    }
    .await;

    match staged {
        Ok(_target) => {
            // 清理下载暂存物（安装成功后无需保留）。
            let _ = std::fs::remove_file(&dest);
            log::info!("[install] 模块 `{id}`@{} 安装完成，重启后生效", entry.version);
            Ok(InstallOutcome {
                id: id.to_string(),
                version: entry.version,
                restart_required: true,
                package_sha256: Some(asset.sha256.clone()),
            })
        }
        Err(e) => {
            // 解包/落位失败：清掉临时目录，绝不留半成品（否则会被磁盘视图当成可用模块）。
            let _ = std::fs::remove_dir_all(&tmp_dir);
            Err(e)
        }
    }
}

/// 轮询等待下载任务结束。
async fn await_download(kernel: &KernelContext, task_id: u64) -> Result<(), KernelError> {
    let started = Instant::now();
    loop {
        if started.elapsed() > DOWNLOAD_TIMEOUT {
            return Err(KernelError::Module("模块包下载超时".into()));
        }
        let Some(snapshot) = kernel.download().task(task_id) else {
            return Err(KernelError::Module("下载任务已消失".into()));
        };
        match snapshot.status {
            DownloadStatus::Done => return Ok(()),
            DownloadStatus::Failed => {
                return Err(KernelError::Module(format!(
                    "模块包下载失败：{}",
                    snapshot.error.unwrap_or_else(|| "未知原因".into())
                )));
            }
            DownloadStatus::Cancelled => {
                return Err(KernelError::Module("模块包下载已取消".into()));
            }
            _ => tokio::time::sleep(DOWNLOAD_POLL_INTERVAL).await,
        }
    }
}

/// 安装用户选择的本地文件。摘要只绑定快照字节，不代表来源已审核。
/// 可被受控集成测试调用；生产命令不暴露 path 参数。
pub fn install_local_package(
    archive: &std::path::Path,
    modules_dir: &std::path::Path,
    validate_target: impl Fn(&str) -> Result<(), KernelError>,
) -> Result<InstallOutcome, KernelError> {
    use std::io::{Read, Write};
    use sha2::{Digest, Sha256};
    let _operation = begin_mutation()?;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    if !archive.extension().is_some_and(|v| v.eq_ignore_ascii_case("cglm")) {
        return Err(KernelError::Module("请选择 .cglm 模块包".into()));
    }
    let mut source = std::fs::File::open(archive)?;
    if !source.metadata()?.is_file() || source.metadata()?.len() > 512 * 1024 * 1024 {
        return Err(KernelError::Module("模块包不是普通文件或超过 512 MiB".into()));
    }
    std::fs::create_dir_all(modules_dir)?;
    let name = format!(".import-{}-{}", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let work = modules_dir.join(name);
    std::fs::create_dir(&work)?;
    let result = (|| {
        let snapshot = work.join("package.cglm");
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&snapshot)?;
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut buf = [0u8; 65536];
        loop {
            let n = source.read(&mut buf)?; if n == 0 { break; }
            total += n as u64;
            if total > 512 * 1024 * 1024 { return Err(KernelError::Module("模块包超过 512 MiB".into())); }
            hash.update(&buf[..n]); file.write_all(&buf[..n])?;
        }
        file.sync_all()?; drop(file);
        let digest = format!("{:x}", hash.finalize());
        let staging = work.join("extracted");
        let extracted = package::extract_package(&snapshot, &staging)?;
        let m = extracted.manifest;
        m.validate()?;
        if !is_valid_module_id(&m.id) || !m.supports_current_platform() || !m.accepts_launcher(env!("CARGO_PKG_VERSION")) {
            return Err(KernelError::Module("模块清单身份、平台或启动器版本不兼容".into()));
        }
        validate_target(&m.id)?;
        let target = modules_dir.join(&m.id);
        if std::fs::symlink_metadata(&target).is_ok_and(|v| v.file_type().is_symlink()) {
            return Err(KernelError::Module("模块目标不能是符号链接".into()));
        }
        package::promote_extracted(&staging, modules_dir, &m.id)?;
        Ok(InstallOutcome { id: m.id, version: m.version, restart_required: true, package_sha256: Some(digest) })
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires COPPER_TEST_CGLM generated by independent template owner"]
    fn installs_external_fixture() {
        let input = std::env::var_os("COPPER_TEST_CGLM").expect("COPPER_TEST_CGLM");
        let root = std::env::temp_dir().join(format!("copper-external-install-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let result = super::install_local_package(std::path::Path::new(&input), &root, |_| Ok(())).unwrap();
        let installed = root.join(&result.id);
        let manifest = crate::registry::manifest::ModuleManifest::parse(&std::fs::read(installed.join("module.json")).unwrap()).unwrap();
        assert_eq!(manifest.id, result.id); assert_eq!(manifest.version, result.version);
        assert!(installed.join("frontend").join(&manifest.frontend.register).is_file());
        use sha2::{Digest, Sha256};
        let expected = format!("{:x}", Sha256::digest(std::fs::read(&input).unwrap()));
        assert_eq!(result.package_sha256.as_deref(), Some(expected.as_str())); assert!(result.restart_required);
        // A fresh disk enumeration sees the promoted id, never .import/.tmp workdirs.
        let scanned: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().filter_map(|v| v.file_name().into_string().ok()).filter(|id| crate::registry::modules::is_valid_module_id(id)).collect();
        assert_eq!(scanned, vec![result.id.clone()]);
        assert!(std::fs::read_dir(&root).unwrap().flatten().all(|v| !v.file_name().to_string_lossy().starts_with('.')));
        println!("real local transaction installed {}@{}; manifest and frontend entry re-read from disk", result.id, result.version);
        std::fs::remove_dir_all(root).unwrap();
    }
}
