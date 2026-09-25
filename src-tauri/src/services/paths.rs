//! 文件系统抽象：集中管理应用路径，避免业务代码绑定 Windows，
//! 为安卓等平台适配留出边界（后续通过条件编译切换实现）。

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use super::settings::SettingsService;
use crate::error::KernelError;

/// 路径体系。
///
/// 布局（以 Windows 为例，`<AppData>` 为 `%APPDATA%`）：
/// - data:      `<AppData>/copper-lamp/copper-golem`（持久数据）
/// - versions:  `data/versions`（已安装游戏版本）
/// - modules:   `data/modules`（附加模块）
/// - cache:     `<LocalAppData>/copper-lamp/copper-golem/cache`（下载缓存）
/// - logs:      `data/logs`（运行日志）
#[derive(Debug, Clone)]
pub struct Paths {
    data_dir: PathBuf,
    versions_dir: PathBuf,
    modules_dir: PathBuf,
    cache_dir: PathBuf,
    logs_dir: PathBuf,
    db_file: PathBuf,
}

impl Paths {
    /// 依据**显式根目录**初始化路径体系（首选入口）。
    ///
    /// 根目录由宿主提供，而非在此处猜测：桌面端传 Tauri 的 app data 目录，
    /// 安卓端传应用私有 `filesDir`。这样 `Paths` 不依赖 `directories` 在
    /// 各平台的行为——`ProjectDirs::from` 在安卓无标准 XDG 目录、通常会返回
    /// `None`，若在此处硬依赖将直接中断启动（见 docs/平台适配.md 3.1 风险 2）。
    ///
    /// 布局：`<root>/data`（持久数据）、`<root>/cache`（下载缓存）。
    pub fn with_root(root: PathBuf) -> Self {
        let data_dir = root.join("data");
        let cache_dir = root.join("cache");
        let versions_dir = data_dir.join("versions");
        let modules_dir = data_dir.join("modules");
        let logs_dir = data_dir.join("logs");
        let db_file = data_dir.join("copper.db");
        Self {
            data_dir,
            versions_dir,
            modules_dir,
            cache_dir,
            logs_dir,
            db_file,
        }
    }

    /// 依据宿主提供的根目录初始化路径体系（应用启动的**唯一**入口）。
    ///
    /// 解析顺序：
    /// 1. `COPPER_DATA_DIR`：开发调试整体重定向数据与缓存根（隔离目录运行）；
    /// 2. 宿主根目录：`app_data_dir()` —— 桌面端为标准用户数据目录，安卓端为
    ///    应用私有 `filesDir`（卸载即清除）。
    ///
    /// 之所以不在此处直接依赖 `directories`：安卓没有标准 XDG 目录，
    /// `ProjectDirs::from` 通常返回 `None`，硬依赖会直接中断启动
    /// （见 docs/平台适配.md 3.1 风险 2）。
    pub fn resolve(app: &AppHandle) -> Result<Self, KernelError> {
        if let Some(custom) = non_empty_env("COPPER_DATA_DIR") {
            let mut paths = Self::with_root(custom);
            paths.logs_dir = Self::resolve_logs_dir_for(&paths.data_dir);
            return Ok(paths);
        }
        let root = app
            .path()
            .app_data_dir()
            .map_err(|e| KernelError::Config(format!("无法解析应用数据目录 app_data_dir: {e}")))?;
        // 桌面端保持既有布局：数据在 `app_data_dir`，缓存在宿主给的缓存目录
        // （`%LOCALAPPDATA%`）下，避免大缓存挤占漫游数据目录。
        let mut paths = Self::with_root(root);
        if let Ok(cache) = app.path().app_cache_dir() {
            paths.cache_dir = cache;
        }
        paths.logs_dir = Self::resolve_logs_dir_for(&paths.data_dir);
        Ok(paths)
    }

    /// 在**已知数据目录**下解析日志目录。
    ///
    /// 除 `COPPER_LOG_DIR` 覆盖外，日志固定落在 `<data_dir>/logs`。
    /// 该解析不依赖 `directories` 探测，也不依赖 [`Paths`] 实例本身：
    /// 日志后端必须在启动最早期（路径体系尚未建好时）就能确定落盘位置。
    pub fn resolve_logs_dir_for(data_dir: &Path) -> PathBuf {
        non_empty_env("COPPER_LOG_DIR").unwrap_or_else(|| data_dir.join("logs"))
    }

    /// 创建全部目录并**校验可写性**（幂等）。
    ///
    /// 这里解决两个此前一直靠猜的问题：
    ///
    /// 1. `create_dir_all` 对**已存在**的目录不做任何写权限校验就返回成功，
    ///    所以「目录存在」不等于「目录可写」。受限环境（安全软件、只读卷、
    ///    收紧的 ACL）下，真正的失败会被推迟到首次落盘那一刻——SQLite 建库、
    ///    写 WAL、写日志——才以裸 `os error 5` 暴露，且不带任何路径信息。
    ///    这里对每个目录补一次写探针，让问题在启动阶段就带着路径报出来。
    /// 2. 目录各有轻重，不该同罪同罚：数据/版本/模块缺失无法继续；日志不可写
    ///    只损失可观测性；下载缓存是可再生成的派生物，回退即可。
    pub fn prepare(&mut self) -> Result<(), KernelError> {
        for (label, dir) in [
            ("数据", &self.data_dir),
            ("版本", &self.versions_dir),
            ("模块", &self.modules_dir),
        ] {
            ensure_writable(dir, label)?;
        }

        // 日志目录不可写只影响可观测性：降级为本次运行不落盘，不中断启动。
        if let Err(e) = ensure_writable(&self.logs_dir, "日志") {
            log::warn!("[paths] 日志目录不可写，本次运行仅输出到 stdout: {e}");
        }

        // 下载缓存是可再生成的派生物：宿主给的缓存卷不可用时回退到数据目录下，
        // 保证下载功能仍然可用（代价是占用数据卷空间）。
        if let Err(e) = ensure_writable(&self.cache_dir, "下载缓存") {
            let fallback = self.data_dir.join("cache");
            log::warn!(
                "[paths] 下载缓存目录不可用，回退到 {}: {e}",
                fallback.display()
            );
            ensure_writable(&fallback, "下载缓存（回退）")?;
            self.cache_dir = fallback;
        }

        Ok(())
    }

    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    /// 已安装游戏版本目录。
    pub fn versions_dir(&self) -> &PathBuf {
        &self.versions_dir
    }

    /// 解析当前游戏（版本）根目录：优先 `settings.game.directory`（非空），否则默认版本目录。
    ///
    /// 是版本根目录的**唯一**解析入口；新增自定义根时由调用方负责 `create_dir_all`。
    pub fn versions_root(&self, settings: &SettingsService) -> PathBuf {
        settings
            .get::<String>("game.directory")
            .filter(|s| !s.trim().is_empty())
            .map(|s| PathBuf::from(s.trim()))
            .unwrap_or_else(|| self.versions_dir.clone())
    }

    /// 附加模块目录。
    pub fn modules_dir(&self) -> &PathBuf {
        &self.modules_dir
    }

    /// 下载缓存目录。
    pub fn cache_dir(&self) -> &PathBuf {
        &self.cache_dir
    }

    /// 日志目录。
    pub fn logs_dir(&self) -> &PathBuf {
        &self.logs_dir
    }

    /// 主数据库文件路径。
    pub fn db_file(&self) -> &PathBuf {
        &self.db_file
    }

    /// 供前端展示 / 调试用的路径快照。
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "data": self.data_dir,
            "versions": self.versions_dir,
            "modules": self.modules_dir,
            "cache": self.cache_dir,
            "logs": self.logs_dir,
            "database": self.db_file,
        })
    }
}

/// 读取非空环境变量：空串视为未设置。
///
/// 不这么处理的话，`COPPER_DATA_DIR=`（空值）会被当成有效路径，把数据目录
/// 静默解析成当前工作目录——这类"看起来生效了"的错误最难排查。
fn non_empty_env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// 创建目录并确认它**真的**可写。
///
/// `create_dir_all` 只保证目录存在，不校验写权限；这里补一次写探针，把
/// 「目录存在但不可写」这种延迟故障提前到启动阶段，并且错误信息里带上路径。
fn ensure_writable(dir: &Path, label: &str) -> Result<(), KernelError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| dir_failure(label, dir, &e))?;

    let probe = dir.join(format!(".write-probe-{}", std::process::id()));
    std::fs::write(&probe, b"").map_err(|e| dir_failure(label, dir, &e))?;
    // 探针清理失败不影响可用性（下次启动会被覆盖写），但值得留痕。
    if let Err(e) = std::fs::remove_file(&probe) {
        log::debug!("[paths] 清理写探针失败 {}: {e}", probe.display());
    }
    Ok(())
}

/// 把裸 `io::Error` 包装成带目录与用途的错误。
///
/// `os error 5` 本身不携带任何位置信息，只能靠人工逐一试路径；这里把
/// 「哪个目录、用来干什么」写进错误消息，让启动失败一次就能定位。
fn dir_failure(label: &str, dir: &Path, e: &std::io::Error) -> KernelError {
    KernelError::Io(std::io::Error::new(
        e.kind(),
        format!("{label}目录 {} 不可用: {e}", dir.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个互不干扰的临时根目录（并行测试下也不互相踩）。
    fn temp_root(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("copper-paths-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("创建临时根目录");
        root
    }

    #[test]
    fn prepare_creates_all_dirs_and_leaves_no_probe() {
        let root = temp_root("ok");
        let mut paths = Paths::with_root(root.clone());
        paths.prepare().expect("prepare 应当成功");

        for dir in [
            paths.data_dir(),
            paths.versions_dir(),
            paths.modules_dir(),
            paths.cache_dir(),
            paths.logs_dir(),
        ] {
            assert!(dir.is_dir(), "目录应已创建: {}", dir.display());
        }

        let leftovers = std::fs::read_dir(paths.data_dir())
            .expect("读取数据目录")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".write-probe-"))
            .count();
        assert_eq!(leftovers, 0, "写探针应被清理，不应留下残渣");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prepare_falls_back_when_cache_dir_is_unusable() {
        let root = temp_root("fallback");
        // 用一个**文件**占住缓存卷的位置，使其下任何子目录都无法创建。
        let blocker = root.join("blocker");
        std::fs::write(&blocker, b"").expect("创建占位文件");

        let mut paths = Paths::with_root(root.join("data-root"));
        paths.cache_dir = blocker.join("cache");
        paths
            .prepare()
            .expect("缓存不可用时应降级而非中断启动");

        assert!(paths.cache_dir().is_dir(), "回退目录应可用");
        assert_eq!(
            paths.cache_dir().as_path(),
            paths.data_dir().join("cache").as_path()
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn blank_env_var_is_treated_as_unset() {
        assert!(non_empty_env("COPPER_PATHS_DEFINITELY_UNSET_VAR").is_none());
    }
}
