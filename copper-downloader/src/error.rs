use std::io;

/// 下载引擎统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("无效参数: {0}")]
    InvalidArgument(String),

    #[error("任务不存在: {0}")]
    TaskNotFound(u64),

    #[error("HTTP 请求失败: {0}")]
    Http(#[from] reqwest::Error),

    #[error("网络错误: {0}")]
    Io(#[from] io::Error),

    #[error("服务器返回状态码 {0}")]
    HttpStatus(u16),

    #[error("下载被取消")]
    Cancelled,

    #[error("下载被暂停")]
    Paused,

    #[error("校验失败: 期望 {expected}，实际 {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("响应缺少长度信息且未提供总大小")]
    UnknownLength,
}

impl DownloadError {
    /// 是否为可安全重试的瞬时性错误。
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            DownloadError::Http(_)
                | DownloadError::Io(_)
                | DownloadError::HttpStatus(500..=599)
        )
    }

    /// 是否为用户主动中断（暂停 / 取消），这类错误不重试、不报错。
    pub fn is_user_interrupt(&self) -> bool {
        matches!(self, DownloadError::Cancelled | DownloadError::Paused)
    }

    /// 是否因用户暂停而中断。
    pub fn is_paused(&self) -> bool {
        matches!(self, DownloadError::Paused)
    }
}

/// 目标文件已存在时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingFilePolicy {
    /// 目标已存在且校验通过则直接视为完成（幂等重试）。
    SkipIfValid,
    /// 强制重新下载覆盖。
    Overwrite,
}
