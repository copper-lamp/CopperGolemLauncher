// 下载 API：投递 / 快照 / 暂停 / 恢复 / 取消 / 重试 / 移除。
//
// 任务进度与状态变化经事件推送（download.created / download.progress /
// download.status），命令层仅用于主动控制与一次性快照。

import { call } from "./core";

export type DownloadStatus =
  | "queued"
  | "downloading"
  | "paused"
  | "cancelled"
  | "failed"
  | "done";

/** 任务快照（与后端 `DownloadTaskView` 同构）。 */
export interface DownloadTask {
  id: number;
  filename: string | null;
  url: string;
  dest: string;
  total_bytes: number;
  downloaded_bytes: number;
  speed_bytes_per_sec: number;
  status: DownloadStatus;
  error: string | null;
  retry_count: number;
}

/** 投递参数（camelCase，全可选）。 */
export interface EnqueueOptions {
  resume?: boolean;
  removeOnCancel?: boolean;
  expectedSha256?: string;
  maxRetries?: number;
  headers?: Array<[string, string]>;
  filename?: string;
  existingPolicy?: "overwrite" | "skip_if_valid";
}

/** 投递下载任务，返回任务 id。 */
export function downloadEnqueue(
  url: string,
  dest: string,
  options?: EnqueueOptions,
): Promise<number> {
  return call<number>("download_enqueue", { url, dest, options });
}

/** 全部任务快照。 */
export function downloadTasks(): Promise<DownloadTask[]> {
  return call<DownloadTask[]>("download_tasks");
}

/** 单个任务快照。 */
export function downloadTask(id: number): Promise<DownloadTask | null> {
  return call<DownloadTask | null>("download_task", { id });
}

export function downloadPause(id: number): Promise<void> {
  return call<void>("download_pause", { id });
}

export function downloadResume(id: number): Promise<void> {
  return call<void>("download_resume", { id });
}

export function downloadCancel(id: number): Promise<void> {
  return call<void>("download_cancel", { id });
}

export function downloadRetry(id: number): Promise<void> {
  return call<void>("download_retry", { id });
}

export function downloadRemove(id: number): Promise<void> {
  return call<void>("download_remove", { id });
}

export function downloadPauseAll(): Promise<void> {
  return call<void>("download_pause_all");
}

export function downloadResumeAll(): Promise<void> {
  return call<void>("download_resume_all");
}

/** 格式化速率（字节/秒 → 可读文本）。 */
export function formatSpeed(bytesPerSec: number): string {
  if (bytesPerSec <= 0) return "0 B/s";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytesPerSec;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[unit]}/s`;
}

/** 格式化字节数。 */
export function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}
