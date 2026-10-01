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
    /// 传输已完成，正在做下载之外的后续阶段（安装 / 解包 / 依赖解析）。
    ///
    /// 该状态与字节进度无关：进度由 [`TaskSnapshot::phase_progress`] 表达，
    /// 文本由 [`TaskSnapshot::stage`] / [`TaskSnapshot::stage_detail`] 表达。
    /// 它**不占用并发名额**（带宽已经不用了），但也不属于终态——清空历史、
    /// 淘汰等操作都必须绕过它。
    Installing,
}

impl DownloadStatus {
    /// 是否处于活跃状态（占用并发名额）。
    pub fn is_active(&self) -> bool {
        matches!(self, DownloadStatus::Queued | DownloadStatus::Downloading)
    }

    /// 是否为终态（不会再自行变化）。
    ///
    /// `Installing` 刻意不算终态：它后面还会变成 `Done` / `Failed`，
    /// 把它当终态会让「清空历史」删掉一条正在安装的条目。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            DownloadStatus::Done
                | DownloadStatus::Failed
                | DownloadStatus::Cancelled
                | DownloadStatus::Paused
        )
    }

    /// 是否为「已收工」的任务（完成 / 失败 / 取消），**不含** `Paused`。
    ///
    /// 「清除下载记录」用它而不是 [`Self::is_terminal`]：暂停态是**可继续**的
    /// 未完成任务，清掉它的记录等于让用户再也点不到「继续」——断点字节虽然还
    /// 在磁盘上，但那条任务身份没了，用户只能重投一次。这是不可逆的损失，
    /// 不该被一个「清理历史」的按钮顺手带走。
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            DownloadStatus::Done | DownloadStatus::Failed | DownloadStatus::Cancelled
        )
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
    /// 阶段化进度（0.0~1.0）。
    ///
    /// `Some` 表示进度不再由字节数决定，而是由外部阶段上报（安装 / 解包等）；
    /// 前端的进度条在有本字段时直接用它，忽略 `total_bytes` / `downloaded_bytes`。
    pub phase_progress: Option<f64>,
    /// 当前阶段文案的 i18n 键（如 `download.stage.extracting`）。
    ///
    /// 用键而非成品文本：文案由前端按当前语言渲染，后端不产出任何面向用户
    /// 的自然语言，避免同一句话在两种语言下出现两种拼写。
    pub stage: Option<String>,
    /// 当前阶段的动态细节（文件名、`32/128` 之类的计数），与 `stage` 拼接展示。
    ///
    /// 这类内容无法预先本地化（来自被解包的文件名、上游守护进程的原始步骤名），
    /// 因此原样透传；为空时前端只显示 `stage` 的译文。
    pub stage_detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归：「清除记录」的判定必须把暂停态与安装态排除在外。
    ///
    /// 暂停是可继续的未完成任务，删掉它的记录等于让用户再也点不到「继续」；
    /// 安装中则后面还会转成终态，清掉会让条目在界面上凭空消失。
    #[test]
    fn finished_excludes_paused_and_installing() {
        assert!(DownloadStatus::Done.is_finished());
        assert!(DownloadStatus::Failed.is_finished());
        assert!(DownloadStatus::Cancelled.is_finished());
        assert!(
            !DownloadStatus::Paused.is_finished(),
            "暂停态不得被「清除记录」带走"
        );
        assert!(!DownloadStatus::Installing.is_finished());
        assert!(!DownloadStatus::Queued.is_finished());
        assert!(!DownloadStatus::Downloading.is_finished());
        // 「终态」与「已收工」刻意不同：暂停确实不会自行变化（是终态），
        // 但它有「继续」这条路可走（不是已收工）。
        assert!(DownloadStatus::Paused.is_terminal());
        assert!(!DownloadStatus::Installing.is_terminal());
    }
}
