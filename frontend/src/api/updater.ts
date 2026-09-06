// 更新 API：检查新版本、下载更新包、安装并重启、查询状态。

import { call } from "./core";

export type UpdatePhase =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "downloaded"
  | "failed";

/** 可用的新版本信息。 */
export interface UpdateInfo {
  version: string;
  notes: string;
  published_at: string;
  asset_name: string;
  asset_size: number;
  download_url: string;
}

/** 更新状态快照。 */
export interface UpdateStatus {
  phase: UpdatePhase;
  current_version: string;
  latest: UpdateInfo | null;
  download_task_id: number | null;
  error: string | null;
}

/** 检查最新版本。 */
export function updaterCheck(): Promise<UpdateStatus> {
  return call<UpdateStatus>("updater_check");
}

/** 下载更新包（返回下载任务 id，进度经 download.* 事件广播）。 */
export function updaterApply(): Promise<number> {
  return call<number>("updater_apply");
}

/** 安装已下载的更新：原地替换并重启。 */
export function updaterInstall(): Promise<void> {
  return call<void>("updater_install");
}

/** 当前更新状态。 */
export function updaterStatus(): Promise<UpdateStatus> {
  return call<UpdateStatus>("updater_status");
}
