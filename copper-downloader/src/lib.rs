//! 铜傀儡启动器独立下载引擎。
//!
//! 设计目标（对应架构总览"下载队列"能力）：
//! - 并发队列：同时下载数可配置，超出部分排队。
//! - 断点续传：写入 `<dest>.part` 临时文件，重试 / 恢复时以 `Range` 请求续传。
//! - 暂停 / 取消 / 恢复：暂停保留临时文件，取消可配置删除临时文件。
//! - 真实进度：已下载字节、总字节、实时速率（指数移动平均）。
//! - 失败重试：瞬时性错误（连接 / 超时 / 5xx）指数退避重试。
//! - 完整性校验：可选期望 SHA-256，下载完成后校验再落位。
//!
//! 该 crate 不依赖 Tauri，内核与模块均可复用；事件经 [`DownloadManager::add_listener`] 向外广播。

pub mod error;
pub mod manager;
pub mod task;

pub use error::DownloadError;
pub use manager::{DownloadEvent, DownloadManager, DownloadOptions};
pub use task::{DownloadStatus, TaskSnapshot};
