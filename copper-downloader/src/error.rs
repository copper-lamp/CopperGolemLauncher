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

    /// 本地文件系统错误。此前与网络错误共用同一文案，导致磁盘满、权限不足、
    /// 路径被占用这类本地故障被显示成「网络错误」，把排查方向带偏；这里单列
    /// 并携带路径与错误码。
    #[error("本地文件错误: {0}")]
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
    ///
    /// 注意：本地文件错误（`Io`）**不**在此列。`PermissionDenied`（Windows 上的
    /// `拒绝访问 os error 5`）、磁盘满、只读卷、目标文件被占用都属于持久性故障，
    /// 重试只会把失败延后若干秒，还会让用户误以为「网络重试中」。这类错误应当
    /// 立即上报，由用户处理占用/权限后再重试。
    pub fn is_transient(&self) -> bool {
        match self {
            DownloadError::Http(_) | DownloadError::HttpStatus(500..=599) => true,
            DownloadError::Io(e) => !is_fatal_io_kind(e.kind()),
            _ => false,
        }
    }

    /// 是否为用户主动中断（暂停 / 取消），这类错误不重试、不报错。
    pub fn is_user_interrupt(&self) -> bool {
        matches!(self, DownloadError::Cancelled | DownloadError::Paused)
    }

    /// 是否因用户暂停而中断。
    pub fn is_paused(&self) -> bool {
        matches!(self, DownloadError::Paused)
    }

    /// 取出底层的本地文件系统错误（非文件错误时返回 None）。
    pub fn as_io(&self) -> Option<&io::Error> {
        match self {
            DownloadError::Io(e) => Some(e),
            _ => None,
        }
    }

    /// 把本地文件错误的 `io::Error` 取回（供需要 `std::io::Result` 的落位流程复用）。
    ///
    /// 仅用于已经确定错误种类为 `Io` 的场景；其它变体退化为 `Other`，不会丢失消息。
    pub fn into_io(self) -> io::Error {
        match self {
            DownloadError::Io(e) => e,
            other => io::Error::other(other.to_string()),
        }
    }
}

/// 本地文件错误中属于「重试无用、必须用户介入」的种类。
fn is_fatal_io_kind(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::PermissionDenied
            | io::ErrorKind::StorageFull
            | io::ErrorKind::ReadOnlyFilesystem
            | io::ErrorKind::InvalidInput
            | io::ErrorKind::NotFound
    )
}

/// 目标文件已存在时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingFilePolicy {
    /// 目标已存在且校验通过则直接视为完成（幂等重试）。
    SkipIfValid,
    /// 强制重新下载覆盖。
    Overwrite,
}
