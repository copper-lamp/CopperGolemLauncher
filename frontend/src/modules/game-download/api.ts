// 游戏下载模块 API：版本清单 / 详情 / 投递下载 / 刷新源 / 取消 / 状态。
//
// 后端命令均为异步、错误经 `KernelApiError` 抛出；模型字段与后端
// Rust `ManifestView` / `TaskView` 保持 camelCase 同构。

import { call } from "../../api/core";

/** 版本类型（与后端 `VersionKind` 对应）。 */
export type GameVersionKind = "release" | "preview";

/** 版本任务状态（与后端 `TaskView.state` 对应）。 */
export type GameDownloadState =
  | "downloading"
  | "extracting"
  | "installed"
  | "failed";

/** 单版本行视图（与后端 `VersionView` 同构，camelCase）。 */
export interface GameVersionView {
  id: string;
  kind: GameVersionKind;
  game_version: string;
  md5: string;
  is_installed: boolean;
  is_downloaded: boolean;
  is_latest: boolean;
  timestamp: number;
  url: string | null;
}

/** 一个大版本分组（与后端 `VersionGroupView` 同构）。 */
export interface GameVersionGroup {
  major: string;
  latest: boolean;
  items: GameVersionView[];
}

/** 前端清单视图（与后端 `ManifestView` 同构）。 */
export interface GameManifestView {
  latest_release: GameVersionView | null;
  latest_preview: GameVersionView | null;
  groups: GameVersionGroup[];
}

/** 下载引擎快照（与后端 `DownloadState` 同构）。 */
export interface GameDownloadSnapshot {
  task_id: number;
  total_bytes: number;
  downloaded_bytes: number;
  speed_bytes_per_sec: number;
}

/** 单版本任务视图（与后端 `TaskView` 同构）。 */
export interface GameTaskView {
  version_id: string;
  kind: GameVersionKind;
  dest: string;
  state: GameDownloadState;
  error: string | null;
  download: GameDownloadSnapshot | null;
}

/** 版本清单（含已安装 / 下载中状态）。 */
export function gameManifest(refresh?: boolean): Promise<GameManifestView> {
  return call<GameManifestView>("game_download_manifest", { refresh });
}

/** 单版本详情（任务状态；无任务返回 null）。 */
export function gameDetail(id: string): Promise<GameTaskView | null> {
  return call<GameTaskView | null>("game_download_detail", { id });
}

/** 投递下载（幂等），返回下载任务 id。 */
export function gameEnqueue(id: string): Promise<number> {
  return call<number>("game_download_enqueue", { id });
}

/** 强制刷新版本清单源。 */
export function gameRefreshSource(): Promise<void> {
  return call<void>("game_download_refresh_source");
}

/** 取消下载任务。 */
export function gameCancel(id: string): Promise<void> {
  return call<void>("game_download_cancel", { id });
}

/** 单版本任务状态。 */
export function gameStatus(id: string): Promise<GameTaskView | null> {
  return call<GameTaskView | null>("game_download_status", { id });
}

/** 格式化字节数（与内核下载页一致）。 */
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

/** 计算下载百分比（0~1）；无总量时返回 0。 */
export function progressRatio(snapshot: GameDownloadSnapshot | null): number {
  if (!snapshot || snapshot.total_bytes <= 0) return 0;
  return Math.min(snapshot.downloaded_bytes / snapshot.total_bytes, 1);
}