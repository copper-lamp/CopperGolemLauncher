// 游戏下载模块 API：版本清单 / 详情 / 投递下载 / 刷新源 / 取消 / 状态。
//
// 后端命令均为异步、错误经 `KernelApiError` 抛出；模型字段与后端
// Rust `ManifestView` / `TaskView` 保持 snake_case 同构。

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

/**
 * 仅重装：整包已在本地时重跑安装流水线，**不重新下载**。
 *
 * 安装阶段失败（商店授权、md5、解包中断）后的补救路径。此前只能
 * `gameEnqueue` 重来，等于把数 GB 的下载重做一遍。
 *
 * 本地整包缺失或校验不符时后端明确报错，不会悄悄改走下载。
 */
export function gameInstall(id: string): Promise<void> {
  return call<void>("game_download_install", { id });
}

/** 下载任务 → 游戏版本的绑定（下载中心据此显示「安装」入口）。 */
export interface GameTaskBinding {
  task_id: number;
  version_id: string;
}

/**
 * 取「下载任务 → 游戏版本」绑定。
 *
 * 下载中心列出的是核心下载任务 id，安装却按版本 id 取记录。这个映射由内核给出，
 * 前端不依据 dest / 文件名猜测——猜错会把安装指向另一个版本。只有确实存在
 * 游戏下载记录的任务才会出现。
 */
export function gameTaskBindings(): Promise<GameTaskBinding[]> {
  return call<GameTaskBinding[]>("game_download_task_bindings");
}

/** APK 导入记录（与后端 `apk::ApkPackageInfo` 同构，camelCase→原样）。 */
export interface ApkPackageInfo {
  package_name: string;
  version_code: number;
  version_name: string;
  abi: string;
  sha256: string;
  has_splits: boolean;
}

/**
 * APK 导入结果（与后端 `apk::ApkImportResult` 同构）。
 *
 * `instance_name` 是内核规整后的**权威实例名**，也就是版本目录名。调用方必须
 * 用它（而不是自己提交的名字）去启动 / 引用实例：安卓宿主按同一个名字定位
 * `data/versions/<name>`，两边各推导一次就会分叉。
 */
export interface ApkImportResult {
  instance_name: string;
  package_info: ApkPackageInfo;
}

/**
 * 导入一个 APK / APKS。
 *
 * `source_path` 必须是应用私有目录内的路径：安卓端先由 `importApkToInbox`
 * 把系统选择器返回的 URI 复制进 `cache/inbox/`。
 *
 * 包名、版本号由后端从二进制 `AndroidManifest.xml` 解码，前端不参与——
 * 这些值决定原生库加载顺序。
 *
 * `name` 会由后端规整（空格 / 中文 / 通配符收敛为 `_`），后续流程请使用返回的
 * `instance_name`。
 */
export function gameImportApk(source_path: string, name: string): Promise<ApkImportResult> {
  return call<ApkImportResult>("game_download_import_apk", { source_path, name });
}

/** 安卓游戏退出记录（与后端 `platform::android::ExitRecord` 同构）。 */
export interface AndroidGameExit {
  instance_name: string;
  reason: string;
  exited_at: number;
}

/**
 * 取走安卓游戏退出记录（take 语义，读后即删）。
 *
 * 桌面端恒返回 `null`，因此调用方无需按平台分支。
 */
export function androidGameTakeExit(): Promise<AndroidGameExit | null> {
  return call<AndroidGameExit | null>("android_game_take_exit");
}

/** SAF 选择结果（与后端 `platform::android::ApkPickResult` 同构）。 */
export interface AndroidApkPickResult {
  request_id: string;
  /** 应用私有目录内的绝对路径；失败时为空。 */
  path: string;
  display_name: string;
  error: string;
}

/**
 * 取走一次 SAF 选择的落盘结果。
 *
 * 返回 `null` 表示安卓宿主尚未写回（仍在系统选择器中，或正在复制）。
 */
export function androidApkPickResult(request_id: string): Promise<AndroidApkPickResult | null> {
  return call<AndroidApkPickResult | null>("android_apk_pick_result", { request_id });
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
