//! 文件系统抽象：集中管理应用路径，避免业务代码绑定 Windows，
//! 为安卓等平台适配留出边界（后续通过条件编译切换实现）。

use std::path::PathBuf;

use directories::ProjectDirs;

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
    /// 依据应用标识初始化路径体系。
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let dirs = ProjectDirs::from("com", "copper-lamp", "CopperGolem")
            .ok_or("failed to resolve project directories")?;
        let data_dir = dirs.data_dir().to_path_buf();
        let versions_dir = data_dir.join("versions");
        let modules_dir = data_dir.join("modules");
        let cache_dir = dirs.cache_dir().to_path_buf();
        let logs_dir = data_dir.join("logs");
        let db_file = data_dir.join("copper.db");
        Ok(Self {
            data_dir,
            versions_dir,
            modules_dir,
            cache_dir,
            logs_dir,
            db_file,
        })
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
