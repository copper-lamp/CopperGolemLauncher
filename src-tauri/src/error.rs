//! 内核统一错误类型：所有服务 / 注册表 / 模块错误在此收敛，
//! 通过 Tauri 命令层序列化后交由前端反馈。

use serde::Serialize;

/// 内核统一错误。
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("无效参数: {0}")]
    InvalidArgument(String),

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("数据库错误: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("序列化错误: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("网络请求失败: {0}")]
    Http(#[from] reqwest::Error),

    #[error("下载错误: {0}")]
    Download(#[from] copper_downloader::DownloadError),

    #[error("密钥存储错误: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("事件总线错误: {0}")]
    EventBus(String),

    #[error("意图错误: {0}")]
    Intent(String),

    #[error("模块错误: {0}")]
    Module(String),

    #[error("账户错误: {0}")]
    Account(String),

    #[error("更新错误: {0}")]
    Updater(String),

    #[error("配置错误: {0}")]
    Config(String),
}

impl KernelError {
    /// 人类可读的短消息（去掉内部技术细节，仅保留首句）。
    pub fn friendly(&self) -> String {
        self.to_string()
    }
}

/// 命令层返回的错误结构（供前端统一 Toast / 弹窗反馈）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CommandError {
    pub kind: String,
    pub message: String,
}

impl CommandError {
    pub fn from_kernel(e: &KernelError) -> Self {
        Self {
            kind: match e {
                KernelError::InvalidArgument(_) => "invalid_argument",
                KernelError::Io(_) => "io",
                KernelError::Database(_) => "database",
                KernelError::Serde(_) => "serde",
                KernelError::Http(_) => "http",
                KernelError::Download(_) => "download",
                KernelError::Keyring(_) => "keyring",
                KernelError::EventBus(_) => "event_bus",
                KernelError::Intent(_) => "intent",
                KernelError::Module(_) => "module",
                KernelError::Account(_) => "account",
                KernelError::Updater(_) => "updater",
                KernelError::Config(_) => "config",
            }
            .to_string(),
            message: e.friendly(),
        }
    }
}

/// 便于命令函数统一返回。
pub type CommandResult<T> = Result<T, CommandError>;
