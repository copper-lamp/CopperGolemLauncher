use std::path::PathBuf;

use serde::Serialize;

/// 任务生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    /// 排队等待（并发上限未满）
    Queued,
    /// 下载中
    Downloading,
    /// 已暂停（保留临时文件，可恢复）
    Paused,
    /// 已取消（临时文件按配置删除）
    Cancelled,
    /// 失败（含错误信息）
    Failed,
    /// 完成（临时文件已落位）
    Done,
}

impl DownloadStatus {
    /// 是否处于活跃状态（占用并发名额）。
    pub fn is_active(&self) -> bool {
        matches!(self, DownloadStatus::Queued | DownloadStatus::Downloading)
    }
}

/// 任务的只读快照，用于状态同步与 UI 渲染。
#[derive(Debug, Clone, Serialize)]
pub struct TaskSnapshot {
    pub id: u64,
    /// 展示用文件名（可为空，回退到 URL 文件名）。
    pub filename: Option<String>,
    pub url: String,
    /// 落位目标路径。
    pub dest: PathBuf,
    /// 临时文件路径（`.part`）。
    pub part_path: PathBuf,
    /// 总字节数，0 表示未知（分块传输等场景，前端显示不确定进度）。
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    /// 实时速率（字节 / 秒，指数移动平均）。
    pub speed_bytes_per_sec: u64,
    pub status: DownloadStatus,
    pub error: Option<String>,
    /// 已重试次数。
    pub retry_count: u32,
    /// 任务创建时间（Unix 毫秒）。
    pub created_at_ms: u64,
}
