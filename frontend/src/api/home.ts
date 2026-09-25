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
  enable_isolation: boolean;
  enable_editor_mode: boolean;
  enable_render_dragon: boolean;
  registered: boolean;
  logo_data_url: string | null;
  /** 版本目录绝对路径（打开文件夹用）。 */
  folder: string;
}

/** 版本设置部分更新（仅覆盖提供的字段；字段名 snake_case 与后端一致）。 */
export interface VersionMetaUpdate {
  enable_editor_mode?: boolean;
  enable_render_dragon?: boolean;
  enable_console?: boolean;
  enable_ctrl_r_reload_resources?: boolean;
  launch_args?: string;
  env_vars?: string;
}

/** 启动结果（与后端 `LaunchOutcome` 对应）。 */
export type LaunchOutcome = "spawned" | "protocol";

/** 内容类型。 */
export type ContentKind = "resources" | "behavior" | "worlds";

/** 内容条目（与后端 `ContentItem` 同构，snake_case）。 */
export interface ContentItem {
  id: string;
  kind: ContentKind;
  name: string;
  enabled: boolean;
  path: string;
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

/** 启动游戏（成功后后台确认进程并广播 `game.launched`）。 */
export function homeLaunch(name: string): Promise<LaunchOutcome> {
  return call<LaunchOutcome>("home_launch", { name });
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

/** 启用 / 禁用内容条目。 */
export function homeContentSetEnabled(
  name: string,
  itemId: string,
  enabled: boolean,
): Promise<void> {
  return call<void>("home_content_set_enabled", { name, itemId, enabled });
}

/** 删除内容条目。 */
export function homeContentRemove(name: string, itemId: string): Promise<void> {
  return call<void>("home_content_remove", { name, itemId });
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
