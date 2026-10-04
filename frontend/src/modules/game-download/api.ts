// 游戏下载模块 API：版本清单 / 加载器 / 实例安装 / 取消 / 重试。
//
// 后端命令均为异步、错误经 `KernelApiError` 抛出；模型字段与后端 Rust 视图保持
// snake_case 同构。命令**参数**名则是 Tauri v2 的 camelCase 约定（见
// `tauri-macros` 的 `ArgumentCase::Camel`），因此这里一律用 camelCase 传参。

import { call } from "../../api/core";

/** 版本类型（与后端 `VersionKind` 对应）。 */
export type GameVersionKind = "release" | "preview";

/** 单版本卡片视图（与后端 `VersionView` 同构）。 */
export interface GameVersionView {
  /** 版本 slug，同时是整包暂存文件名。 */
  id: string;
  kind: GameVersionKind;
  /** 数值版本号（如 `1.21.130.22`）。 */
  game_version: string;
  /** 该版本是否有可装的 LeviLamina（列表页据此显示加载器徽标）。 */
  has_loader: boolean;
  /**
   * 版本安装目录下是否已有该版本的完整整包（列表页据此显示「已下载」徽标）。
   *
   * 由后端**扫目录**得出而不是查数据库记录：记录会与磁盘分叉——用户手动删了包
   * 记录还在，界面就会一直谎称「已下载」；用户手动放了包记录却不在，界面又会
   * 让用户重下几个 G。存在性只有一个事实源，就是那个文件。
   */
  downloaded: boolean;
}

/** 前端清单视图（与后端 `ManifestView` 同构）：两个平表，各自新→旧。 */
export interface GameManifestView {
  latest_release: GameVersionView | null;
  latest_preview: GameVersionView | null;
  releases: GameVersionView[];
  previews: GameVersionView[];
}

/** 加载器候选版本（与后端 `loader_catalog::LoaderOption` 同构）。 */
export interface LoaderOption {
  version: string;
  /**
   * 是否与该游戏版本匹配。
   *
   * 可用性由后端的 LeviLamina 版本库（`levilamina-client-version-db`，键即 MCBE 版本）
   * 判定，后端只返回该游戏版本真正可用的加载器，因此这里恒为 `true`。
   */
  compatible: boolean;
}

/** 某版本的加载器清单（与后端 `LoaderOptions` 同构）。 */
export interface LoaderOptions {
  /** 本机是否有 lipd；为假时选中的加载器装不上，前端应提前提示。 */
  lip_available: boolean;
  loaders: LoaderOption[];
}

/**
 * 实例名不可用原因码（与后端 `home::meta::NameRejection` 同构）。
 *
 * 文案由前端 i18n 按码取词：后端只判定，不做本地化。
 */
export type InstanceNameRejection =
  | "empty"
  | "too_long"
  | "trailing_dot_or_space"
  | "illegal_char"
  | "control_char"
  | "reserved"
  | "taken";

/** 实例名检查结果（与后端 `InstanceCheck` 同构）。 */
export interface InstanceCheck {
  /** 规整后的实例名；提交时用它，而不是用户原始输入。 */
  name: string;
  available: boolean;
  reason: InstanceNameRejection | null;
}

/** 版本清单（含每个版本的加载器可用性）。 */
export function gameManifest(refresh?: boolean): Promise<GameManifestView> {
  return call<GameManifestView>("game_download_manifest", { refresh });
}

/** 某版本可选的加载器清单（详情页「加载器」下拉）。 */
export function gameLoaders(id: string): Promise<LoaderOptions> {
  return call<LoaderOptions>("game_download_loaders", { id });
}

/**
 * 为该版本推荐一个可用实例名（安装确认弹窗初值）。
 *
 * `loader` 非空时后端会把 `-LeviLamina` 追加进默认名：实例名同时是版本目录名，
 * 带不带加载器必须是两个不同目录。前端不自己拼后缀——名字的唯一权威在后端，
 * 两边各拼一次迟早漂移。
 */
export function gameInstanceSuggest(id: string, loader?: string | null): Promise<string> {
  return call<string>("game_download_instance_suggest", { id, loader: loader ?? null });
}

/** 实例名可用性检查（弹窗输入即时反馈）。 */
export function gameInstanceCheck(name: string): Promise<InstanceCheck> {
  return call<InstanceCheck>("game_download_instance_check", { name });
}

/**
 * 以指定实例名安装某版本，返回整包下载任务 id。
 *
 * 返回 `0` 表示整包已在本地、无需下载（安装已经开始）。
 * 同一个版本可以用不同实例名安装任意多次，实例之间完全隔离。
 */
export function gameInstall(
  id: string,
  instance: string,
  loader?: string | null,
): Promise<number> {
  return call<number>("game_download_install", {
    id,
    instance,
    loader: loader ?? null,
  });
}

/** 实例级重试安装：整包在本地时**不重新下载**。 */
export function gameRetry(instance: string): Promise<void> {
  return call<void>("game_download_retry", { instance });
}

/** 版本级重试：把该版本下所有未装好的实例重新排进安装（下载中心的安装入口）。 */
export function gameRetryVersion(id: string): Promise<void> {
  return call<void>("game_download_retry_version", { id });
}

/** 取消一次实例安装（该版本再无待装实例时连整包下载一起放弃）。 */
export function gameCancel(instance: string): Promise<void> {
  return call<void>("game_download_cancel", { instance });
}

/** 下载任务 → 游戏版本的绑定（下载中心据此显示「安装」入口）。 */
export interface GameTaskBinding {
  task_id: number;
  version_id: string;
}

/**
 * 取「下载任务 → 游戏版本」绑定。
 *
 * 下载中心列出的是核心下载任务 id，安装却按版本取待装实例。这个映射由内核给出，
 * 前端不依据 dest / 文件名猜测——猜错会把安装指向另一个版本。只有确实存在游戏下载
 * 记录的任务才会出现。
 */
export function gameTaskBindings(): Promise<GameTaskBinding[]> {
  return call<GameTaskBinding[]>("game_download_task_bindings");
}

/** APK 导入记录（与后端 `apk::ApkPackageInfo` 同构）。 */
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
 * `sourcePath` 必须是应用私有目录内的路径：安卓端先由 `importApkToInbox`
 * 把系统选择器返回的 URI 复制进 `cache/inbox/`。
 *
 * 包名、版本号由后端从二进制 `AndroidManifest.xml` 解码，前端不参与——
 * 这些值决定原生库加载顺序。
 *
 * `name` 会由后端规整（空格 / 中文 / 通配符收敛为 `_`），后续流程请使用返回的
 * `instance_name`。
 */
export function gameImportApk(sourcePath: string, name: string): Promise<ApkImportResult> {
  return call<ApkImportResult>("game_download_import_apk", { sourcePath, name });
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
export function androidApkPickResult(requestId: string): Promise<AndroidApkPickResult | null> {
  return call<AndroidApkPickResult | null>("android_apk_pick_result", { requestId });
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
