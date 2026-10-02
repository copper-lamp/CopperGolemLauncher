// 更新 API：查询状态、检查、下载、取消、安装重启。
//
// 命令层与后端 `services/updater.rs` 一一对应；这里只做类型化封装，
// 不缓存任何状态——状态一律以 `update-status` 事件为准（见 `useUpdate.ts`）。

import { call } from "./core";

/** 更新流程阶段。 */
export type UpdatePhase =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "downloaded"
  | "failed";

/** 失败来源分类，决定文案与可重试动作。 */
export type UpdateErrorKind = "network" | "no_asset" | "download";

/** 更新包形态：决定安装走哪条替换路径。 */
export type UpdateKind = "portable" | "nsis" | "appimage" | "manual";

/** 新版本信息。 */
export interface UpdateInfo {
  version: string;
  tag: string;
  notes: string;
  published_at: string;
  html_url: string;
  asset_name: string;
  asset_size: number;
  download_url: string;
  kind: UpdateKind;
  sha256?: string;
  sha256_url?: string;
}

/** 更新状态快照。 */
export interface UpdateStatus {
  phase: UpdatePhase;
  current_version: string;
  latest: UpdateInfo | null;
  download_task_id: number | null;
  error: string | null;
  error_kind: UpdateErrorKind | null;
  /** 是否处于「下载中 / 已就绪」这两个需要持续展示进度的阶段。 */
  active: boolean;
  last_checked_at: number | null;
}

/** 当前更新状态。 */
export function updaterStatus(): Promise<UpdateStatus> {
  return call<UpdateStatus>("updater_status");
}

/**
 * 手动检查新版本。
 *
 * 失败时后端返回错误原文，前端直接展示——用户主动点的按钮没有提示就是坏了。
 */
export function updaterCheck(): Promise<UpdateStatus> {
  return call<UpdateStatus>("updater_check");
}

/** 把更新包投进全局下载队列，返回下载任务 id。 */
export function updaterDownload(): Promise<number> {
  return call<number>("updater_download");
}

/** 取消更新包下载（保留断点，可再次续传）。 */
export function updaterCancel(): Promise<void> {
  return call<void>("updater_cancel");
}

/** 执行替换并重启启动器。成功即进程退出，不再返回。 */
export function updaterInstall(): Promise<void> {
  return call<void>("updater_install");
}

/** 该形态能否自动安装（`manual` 需用户自行下载安装）。 */
export function isAutoInstallable(kind: UpdateKind | undefined): boolean {
  return kind === "portable" || kind === "nsis" || kind === "appimage";
}