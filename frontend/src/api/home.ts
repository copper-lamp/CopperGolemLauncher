// 开始页模块 API：版本清单 / 版本设置 / 启动游戏 / 封面 / 内容管理。
//
// 注意：Tauri v2 命令参数默认转 camelCase（`old_name` → `oldName`）；
// 结构体字段按各自 serde 约定（`VersionView` / `ContentItem` 为 snake_case，
// `VersionMetaUpdate` 为 snake_case），与后端声明一致。

import { call } from "./core";

/** 版本类型（与后端 `VersionMeta.version_type` 对应）。 */
export type VersionType = "release" | "preview" | "beta";

/** 版本清单视图（与后端 `VersionView` 同构，snake_case）。 */
export interface VersionView {
  name: string;
  game_version: string;
  version_type: string;
  /** 恒为 true：隔离强制开启，没有关闭入口。 */
  enable_isolation: boolean;
  enable_editor_mode: boolean;
  /** 以控制台子系统启动（后端改写游戏 exe 的 PE Subsystem，下次启动生效）。 */
  enable_console: boolean;
  /** 该游戏版本是否支持编辑器模式；false 时前端应禁用该开关。 */
  editor_supported: boolean;
  registered: boolean;
  /** 该实例已安装的加载器版本；null = 未装。 */
  loader: string | null;
  /** 实例模式：original / levi / client（旧实例按 loader 推导）。 */
  mode: string;
  /** 实例创建时间（RFC3339；展示时只取日期部分）。 */
  created_at: string;
  /** 客户端模式选中的客户端库记录 id。 */
  client_id: string | null;
  /** 旧版客户端 DLL 路径（已弃用，仅兼容旧数据）。 */
  client_dll: string | null;
  logo_data_url: string | null;
  /** 版本目录绝对路径（打开文件夹用）。 */
  folder: string;
}

/** 版本设置部分更新（仅覆盖提供的字段；字段名 snake_case 与后端一致）。 */
export interface VersionMetaUpdate {
  enable_editor_mode?: boolean;
  enable_console?: boolean;
  mode?: string;
  client_id?: string;
  client_dll?: string;
  launch_args?: string;
  env_vars?: string;
}

/** 启动结果（与后端 `LaunchOutcome` 对应）。 */
export type LaunchOutcome = "spawned" | "protocol";

/** 内容类型。 */
/** 内容类别（与后端 `home::content::ContentKind` 同构）。
 *
 * `templates` 对应 `development_behavior_packs`：MCBE 不自动加载它，
 * 但内容下载装进去的模板必须看得见，否则用户会以为「装完就消失」。
 */
export type ContentKind =
  | "resources"
  | "behavior"
  | "worlds"
  | "templates"
  | "screenshots";

/** 内容条目（与后端 `ContentItem` 同构，snake_case）。 */
export interface ContentItem {
  id: string;
  kind: ContentKind;
  name: string;
  path: string;
  /** 目录还是单个文件（压缩包、截图）。 */
  is_dir: boolean;
  /** 字节数；读取失败时为 null。 */
  size: number | null;
  /** 修改时间，Unix 毫秒；读取失败时为 null。 */
  modified: number | null;
}

/** 默认模式列表区条目类型。 */
export type DashboardKind = "world" | "server";

/** 默认模式列表区条目（与后端 `DashboardEntry` 同构）。 */
export interface DashboardEntry {
  id: string;
  kind: DashboardKind;
  name: string;
  version: string;
  detail: string;
  /** 封面（`images` 的第一张）；服务器为 null。 */
  image_data_url: string | null;
  /** 该条目全部可用预览图；服务器为空数组。 */
  images: string[];
}

/** 跨实例扫描本地世界与外部服务器。 */
export function homeDashboardEntries(): Promise<DashboardEntry[]> {
  return call<DashboardEntry[]>("home_dashboard_entries");
}

/** 版本清单（按创建时间倒序，新版本在前）。 */
export function homeVersionsList(): Promise<VersionView[]> {
  return call<VersionView[]>("home_versions_list");
}

/** 当前解析的游戏（版本）根目录（含自定义 `game.directory`）。 */
export function homeVersionsRoot(): Promise<string> {
  return call<string>("home_versions_root");
}

/** 单个版本信息。 */
export function homeVersionGet(name: string): Promise<VersionView> {
  return call<VersionView>("home_version_get", { name });
}

/** 部分更新版本设置，返回更新后的视图。 */
export function homeVersionSaveMeta(
  name: string,
  update: VersionMetaUpdate,
): Promise<VersionView> {
  return call<VersionView>("home_version_save_meta", { name, update });
}

/** 重命名版本（目录 + 元数据）。 */
export function homeVersionRename(
  oldName: string,
  newName: string,
): Promise<VersionView> {
  return call<VersionView>("home_version_rename", { oldName, newName });
}

/** 删除版本（游戏运行中后端拒绝；成功后广播 `version.removed`）。 */
export function homeVersionDelete(name: string): Promise<void> {
  return call<void>("home_version_delete", { name });
}

/** 启动游戏（成功后后台确认进程并广播 `game.launched` / `game.exited`）。 */
export function homeLaunch(name: string): Promise<LaunchOutcome> {
  return call<LaunchOutcome>("home_launch", { name });
}

/** 该版本的游戏进程是否在运行（开始页按钮形态的依据）。 */
export function homeGameRunning(name: string): Promise<boolean> {
  return call<boolean>("home_game_running", { name });
}

/** 结束该版本运行中的游戏进程，返回结束的进程数；未在运行时返回 0。 */
export function homeGameKill(name: string): Promise<number> {
  return call<number>("home_game_kill", { name });
}

/** 启动文件状态（hook 是否落位、导入是否生效、原始备份是否存在）。 */
export interface LaunchFileState {
  hook_dll_present: boolean;
  /** exe 导入表里已有 hook —— true 才代表隔离 / 加载器注入在生效。 */
  injected: boolean;
  backup_exists: boolean;
  backup_bytes: number;
  console: boolean;
}

export function homeLaunchFileState(name: string): Promise<LaunchFileState> {
  return call<LaunchFileState>("home_launch_file_state", { name });
}

/** 还原为未注入的原始启动文件。 */
export function homeLaunchFileRestore(name: string): Promise<void> {
  return call<void>("home_launch_file_restore", { name });
}

/** 删除原始启动文件备份。 */
export function homeLaunchBackupDelete(name: string): Promise<void> {
  return call<void>("home_launch_backup_delete", { name });
}

/** 预加载清单探测摘要。 */
export interface PreloadSummary {
  entry_count: number;
  loader_name: string | null;
  /** 被外部预加载器接管时为 true（此时本启动器不加载任何原生模块）。 */
  external_preloader: boolean;
}

export function homePreloadSummary(name: string): Promise<PreloadSummary> {
  return call<PreloadSummary>("home_preload_summary", { name });
}

/** 保存版本封面（`data:image/png;base64,...`，前端已裁剪 256×256 方形）。 */
export function homeLogoSet(name: string, dataUrl: string): Promise<void> {
  return call<void>("home_logo_set", { name, dataUrl });
}

/** 移除版本封面。 */
export function homeLogoRemove(name: string): Promise<void> {
  return call<void>("home_logo_remove", { name });
}

/** 版本已加入资源清单。 */
export function homeContentList(name: string): Promise<ContentItem[]> {
  return call<ContentItem[]>("home_content_list", { name });
}

/** 删除内容条目。 */
export function homeContentRemove(name: string, itemId: string): Promise<void> {
  return call<void>("home_content_remove", { name, itemId });
}

/** 内容条目预览图（data URL）；没有图标时返回 null。 */
export function homeContentPreview(
  name: string,
  itemId: string,
): Promise<string | null> {
  return call<string | null>("home_content_preview", { name, itemId });
}

/** 在文件管理器中定位内容条目，返回其绝对路径。 */
export function homeContentOpenFolder(
  name: string,
  itemId: string,
): Promise<string> {
  return call<string>("home_content_open_folder", { name, itemId });
}

// ---------------------------------------------------------------- 模组管理

/** 模组视图（与后端 `ModView` 同构，snake_case）。 */
export interface ModView {
  /** 模组文件夹名（后续操作的句柄）。 */
  folder: string;
  name: string;
  version: string;
  mod_type: string;
  author: string;
  entry: string;
  enabled: boolean;
  /** 模组文件夹绝对路径。 */
  path: string;
}

/** 模组清单结果（`skipped` 为缺少清单被跳过的目录数）。 */
export interface ModListResult {
  mods: ModView[];
  skipped: number;
}

/** 模组清单。 */
export function homeModsList(name: string): Promise<ModListResult> {
  return call<ModListResult>("home_mods_list", { name });
}

/** 可更新的模组（只有来自内容目录、且目录里有更新版本的模组会返回）。 */
export interface ModUpdate {
  folder: string;
  latest_version: string;
}

/**
 * 检查可更新的模组。
 *
 * 手动导入的 ZIP / DLL 没有来源信息，不会出现在结果里；调用方按 `folder` 匹配清单即可。
 * 这是**网络请求**，列表应先渲染再补徽标。
 */
export function homeModsUpdates(name: string): Promise<ModUpdate[]> {
  return call<ModUpdate[]>("home_mods_updates", { name });
}

/** 从 ZIP 导入模组；重名且未显式覆盖时抛 `KernelApiError`（`kind === "conflict"`）。 */
export function homeModsImportZip(
  name: string,
  sourcePath: string,
  overwrite = false,
): Promise<ModView> {
  return call<ModView>("home_mods_import_zip", { name, sourcePath, overwrite });
}

/** 从单个 DLL 导入模组（自动生成清单）。 */
export function homeModsImportDll(
  name: string,
  sourcePath: string,
  modName: string,
  modType: string,
  version: string,
  overwrite = false,
): Promise<ModView> {
  return call<ModView>("home_mods_import_dll", {
    name,
    sourcePath,
    modName,
    modType,
    version,
    overwrite,
  });
}

/** 启用 / 停用模组。 */
export function homeModsSetEnabled(
  name: string,
  folder: string,
  enabled: boolean,
): Promise<void> {
  return call<void>("home_mods_set_enabled", { name, folder, enabled });
}

/** 删除模组（移除整个模组文件夹）。 */
export function homeModsRemove(name: string, folder: string): Promise<void> {
  return call<void>("home_mods_remove", { name, folder });
}

/** 编辑模组清单，返回更新后的视图。 */
export function homeModsSaveManifest(
  name: string,
  folder: string,
  modName: string,
  entry: string,
  version: string,
  modType: string,
  author: string,
): Promise<ModView> {
  return call<ModView>("home_mods_save_manifest", {
    name,
    folder,
    modName,
    entry,
    version,
    modType,
    author,
  });
}

/** 在系统文件管理器中打开模组目录，返回目录绝对路径。 */
export function homeModsOpenFolder(name: string): Promise<string> {
  return call<string>("home_mods_open_folder", { name });
}

/** 版本目录快捷方式的目标。 */
export type VersionDirKind = "version" | "mods" | "worlds";

/** 在系统文件管理器中打开版本相关目录（版本 / 模组 / 存档），返回目录绝对路径。 */
export function homeVersionOpenDir(name: string, kind: VersionDirKind): Promise<string> {
  return call<string>("home_version_open_dir", { name, kind });
}

// ---------------------------------------------------------------- 屏幕触控层

/**
 * 控件类型（与 Rust `home::controls` 的常量一致）。
 *
 * `dpad` / `joystick` 绑定方向动作，`button` 绑定单动作，`look` 是视角区。
 */
export type ControlKind = "dpad" | "joystick" | "button" | "look";

/**
 * 控件绑定的**游戏动作**（不是按键码）。
 *
 * 用动作而非按键码：按键码是平台细节，写进用户布局会把布局绑死在某个平台上。
 */
export type ControlAction =
  | "forward"
  | "back"
  | "left"
  | "right"
  | "jump"
  | "sneak"
  | "sprint"
  | "inventory"
  | "drop"
  | "attack"
  | "use"
  | "toggle_fly"
  | "chat"
  | "menu";

/** 归一化矩形：0..1，相对屏幕；换机与旋转后仍能还原。 */
export interface ControlRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface ControlItem {
  /** 实例内唯一 id；增删改用它定位，不用下标。 */
  id: string;
  kind: ControlKind;
  rect: ControlRect;
  /** 显示名；空则按 action 取默认缩写。 */
  label: string;
  action: ControlAction | null;
  opacity: number;
  visible: boolean;
}

/** 一份触控布局（与后端 `controls::ControlLayout` 同构）。 */
export interface ControlLayout {
  version: number;
  coordinateSpace: string;
  controls: ControlItem[];
}

/** 方向动作（只能绑在 dpad / joystick 上）。 */
export const DIRECTIONAL_ACTIONS: ControlAction[] = ["forward", "back", "left", "right"];

/** 单动作（只能绑在 button 上）。 */
export const BUTTON_ACTIONS: ControlAction[] = [
  "jump",
  "sneak",
  "sprint",
  "inventory",
  "drop",
  "attack",
  "use",
  "toggle_fly",
  "chat",
  "menu",
];

/**
 * 读取某实例的屏幕触控布局。
 *
 * 布局文件缺失时后端返回默认布局（**不落盘**），因此这里恒能拿到可用布局。
 */
export function homeControlsGet(name: string): Promise<ControlLayout> {
  return call<ControlLayout>("home_controls_get", { name });
}

/**
 * 保存某实例的屏幕触控布局。
 *
 * 后端会做完整校验（类型与动作匹配、id 唯一、坐标夹取）并**原子写**，返回规整后
 * 的布局——请用返回值刷新界面，而不是沿用提交的那份。
 */
export function homeControlsSave(name: string, layout: ControlLayout): Promise<ControlLayout> {
  return call<ControlLayout>("home_controls_save", { name, layout });
}
// ---------------------------------------------------------------- 客户端库

/** 启动器管理的第三方客户端（与后端 module.home.client.ClientView 同构）。 */
export interface ClientView {
  id: string;
  name: string;
  file_name: string;
  sha256: string;
  size: number;
  imported_at: number;
  /** 客户端 DLL 的绝对路径。 */
  path: string;
  /** 文件是否还在客户端目录里。 */
  available: boolean;
}

/** 一次客户端注入的结果（事件 client-inject_result 的负载）。 */
export interface ClientInjectResult {
  instance: string;
  client_id: string;
  client_name: string;
  success: boolean;
  error: string | null;
  source: "auto" | "manual" | string;
  /** Unix 毫秒时间戳。 */
  at: number;
}

/** 本地客户端库列表（开始页模块内部，Windows 专属）。 */
export function homeClientsList(): Promise<ClientView[]> {
  return call<ClientView[]>("home_clients_list");
}

/** 导入客户端 DLL（会复制进启动器数据目录并写库）。 */
export function homeClientImport(sourcePath: string, name: string): Promise<ClientView> {
  return call<ClientView>("home_client_import", { sourcePath, name });
}

/** 删除客户端（数据库记录 + 文件；游戏正在使用该 DLL 时会失败）。 */
export function homeClientDelete(id: string): Promise<void> {
  return call<void>("home_client_delete", { id });
}

/** 游戏运行期间重新注入该实例选中的客户端（结果经事件返回）。 */
export function homeClientReinject(name: string): Promise<void> {
  return call<void>("home_client_reinject", { name });
}