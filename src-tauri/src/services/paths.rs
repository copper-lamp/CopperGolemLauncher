//! 文件系统抽象：集中管理应用路径，避免业务代码绑定 Windows，
//! 为安卓等平台适配留出边界（后续通过条件编译切换实现）。

use std::path::PathBuf;

use directories::ProjectDirs;

use super::settings::SettingsService;

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

    /// 依据应用标识初始化路径体系（无宿主根目录时的兜底）。
    ///
    /// 解析顺序：
    /// 1. `COPPER_DATA_DIR`：开发调试整体重定向数据与缓存根（隔离目录运行）；
    /// 2. `directories::ProjectDirs`：桌面平台的标准用户目录。
    ///
    /// 安卓等无 `ProjectDirs` 的平台必须走 [`Paths::with_root`]；若走到此处
    /// 且解析失败，返回错误由调用方决定是否降级。
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // 开发调试可用 COPPER_DATA_DIR 整体重定向数据与缓存根目录，
        // 便于在受限环境或隔离目录中运行而不污染真实用户数据。
        if let Some(custom) = std::env::var_os("COPPER_DATA_DIR") {
            if !custom.is_empty() {
                let mut paths = Self::with_root(PathBuf::from(custom));
                paths.logs_dir = Self::resolve_logs_dir()?;
                return Ok(paths);
            }
        }
        let dirs = ProjectDirs::from("com", "copper-lamp", "CopperGolem")
            .ok_or("failed to resolve project directories")?;
        // 桌面端：数据与缓存分属不同根（`%APPDATA%` / `%LOCALAPPDATA%`），
        // 不能复用 `with_root`（它把两者放在同一根下）。
        let data_dir = dirs.data_dir().to_path_buf();
        let cache_dir = dirs.cache_dir().to_path_buf();
        Ok(Self {
            versions_dir: data_dir.join("versions"),
            modules_dir: data_dir.join("modules"),
            logs_dir: Self::resolve_logs_dir()?,
            db_file: data_dir.join("copper.db"),
            data_dir,
            cache_dir,
        })
    }

    /// 解析日志目录，不依赖 `Paths` 实例。
    ///
    /// 与 [`Paths::new`] 共用同一套解析规则，保证两处永远一致。
    pub fn resolve_logs_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
        // 开发调试可用 COPPER_LOG_DIR 覆盖日志落盘位置（例如把日志固定到仓库内）。
        if let Some(custom) = std::env::var_os("COPPER_LOG_DIR") {
            if !custom.is_empty() {
                return Ok(PathBuf::from(custom));
            }
        }
        let dirs = ProjectDirs::from("com", "copper-lamp", "CopperGolem")
            .ok_or("failed to resolve project directories")?;
        Ok(dirs.data_dir().join("logs"))
    }

    /// 创建全部目录（幂等）。
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [
            &self.data_dir,
            &self.versions_dir,
            &self.modules_dir,
            &self.cache_dir,
            &self.logs_dir,
        ] {
            std::fs::create_dir_all(dir)?;
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
