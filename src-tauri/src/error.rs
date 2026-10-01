//! 内核统一错误类型：所有服务 / 注册表 / 模块错误在此收敛，
//! 通过 Tauri 命令层序列化后交由前端反馈。

use serde::Serialize;

/// 内核统一错误。
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("无效参数: {0}")]
    InvalidArgument(String),

    #[error("冲突: {0}")]
    Conflict(String),

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
    Secret(String),

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

    /// 启动装配失败：`step` 标出失败的步骤，`source` 保留底层原因。
    ///
    /// setup 阶段此前一律用 `?` 冒泡，最终被 Tauri 压成一句
    /// `Failed to setup app: ...`，既看不出是哪一步，也看不出涉及哪个路径。
    /// 包一层步骤名后，启动失败一次就能定位。
    #[error("启动失败于「{step}」: {source}")]
    Startup {
        step: String,
        source: Box<KernelError>,
    },
}

impl KernelError {
    /// 为启动装配步骤附加步骤名（保留底层原因）。
    pub fn startup(step: impl Into<String>, source: KernelError) -> Self {
        Self::Startup {
            step: step.into(),
            source: Box::new(source),
        }
    }

    /// 人类可读的短消息（去掉内部技术细节，仅保留首句）。
    pub fn friendly(&self) -> String {
        self.to_string()
    }

    /// 错误**载荷**消息：剥掉 `IO 错误:` / `无效参数:` 这类变体前缀，只留正文。
    ///
    /// 二次包装错误时若直接 `format!("{error}…")`，前缀会被套两遍，实测日志里
    /// 出现过 `IO 错误: IO 错误: 版本根目录 … 不可用`。包装方应改用本方法，
    /// 由**外层**变体提供唯一前缀。
    pub fn payload(&self) -> String {
        match self {
            Self::InvalidArgument(m)
            | Self::Conflict(m)
            | Self::Secret(m)
            | Self::Module(m)
            | Self::Account(m)
            | Self::Updater(m)
            | Self::Config(m) => m.clone(),
            Self::Io(e) => e.to_string(),
            Self::Startup { step, source } => {
                format!("启动失败于「{step}」: {}", source.payload())
            }
            Self::Database(e) => e.to_string(),
            Self::Serde(e) => e.to_string(),
            Self::Http(e) => e.to_string(),
            Self::Download(e) => e.to_string(),
            Self::EventBus(m) | Self::Intent(m) => m.clone(),
        }
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
                KernelError::Conflict(_) => "conflict",
                KernelError::Io(_) => "io",
                KernelError::Database(_) => "database",
                KernelError::Serde(_) => "serde",
                KernelError::Http(_) => "http",
                KernelError::Download(_) => "download",
                KernelError::Secret(_) => "keyring",
                KernelError::EventBus(_) => "event_bus",
                KernelError::Intent(_) => "intent",
                KernelError::Module(_) => "module",
                KernelError::Account(_) => "account",
                KernelError::Updater(_) => "updater",
                KernelError::Config(_) => "config",
                KernelError::Startup { .. } => "startup",
            }
            .to_string(),
            message: e.friendly(),
        }
    }
}

/// 便于命令函数统一返回。
pub type CommandResult<T> = Result<T, CommandError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归：二次包装错误时直接用 `{error}` 会把变体前缀套两遍。`payload` 必须只给正文。
    #[test]
    fn payload_strips_variant_prefix() {
        let io = KernelError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "拒绝访问。 (os error 5)",
        ));
        assert_eq!(io.payload(), "拒绝访问。 (os error 5)");
        assert_eq!(
            KernelError::InvalidArgument("版本已安装".into()).payload(),
            "版本已安装"
        );

        let wrapped = KernelError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{}（该路径由设置项控制）", io.payload()),
        ));
        assert_eq!(
            wrapped.to_string().matches("IO 错误:").count(),
            1,
            "包装后前缀只应出现一次: {wrapped}"
        );
    }

    /// `Startup` 是唯一带 step 的变体，`payload` 展开后不应丢步骤名，也不应重复前缀。
    #[test]
    fn payload_keeps_startup_step() {
        let e = KernelError::startup(
            "准备数据目录",
            KernelError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "拒绝访问。 (os error 5)",
            )),
        );
        assert_eq!(
            e.payload(),
            "启动失败于「准备数据目录」: 拒绝访问。 (os error 5)"
        );
    }
}
