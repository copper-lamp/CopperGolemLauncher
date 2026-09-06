// 内容下载模块 API：列表 / 详情 / readme / 下载投递 / lip 安装。
//
// 后端命令均为异步、错误经 `KernelApiError` 抛出；模型字段与后端
// `model.rs` 保持 camelCase 同构。

import { call } from "../../api/core";

/** 来源（与后端 `SOURCE_*` 对应）。 */
export type ContentSource = "curseforge" | "lip";

/** 内容类型（与后端 `TYPE_*` 对应）。 */
export type ContentType =
  | "behavior_pack"
  | "texture_pack"
  | "shader"
  | "ll_mod";

/** 列表卡片（与后端 `ContentItem` 同构，camelCase）。 */
export interface ContentItem {
  id: string;
  source: ContentSource;
  content_type: ContentType;
  name: string;
  description: string;
  author: string | null;
  icon_url?: string | null;
  categories: string[];
  min_game_version: string | null;
  max_game_version: string | null;
  latest_version: string;
  download_count: number;
}

/** 列表返回（与后端 `ContentListPage` 同构）。 */
export interface ContentListPage {
  items: ContentItem[];
  has_more: boolean;
  total: number;
}

/** 列表查询参数（camelCase）。 */
export interface ContentListQuery {
  source?: ContentSource;
  content_type?: ContentType;
  search?: string;
  page?: number;
}

/** 详情内一个可下载文件 / 版本（与后端 `ContentFile` 同构）。 */
export interface ContentFile {
  id: string;
  version: string;
  filename: string;
  download_url: string;
  size: number;
  sha256?: string | null;
  game_versions: string[];
  dependencies: ContentDependency[];
  release_type: string;
}

/** 依赖（与后端 `ContentDependency` 同构）。 */
export interface ContentDependency {
  ref_id: string;
  name: string;
  kind: "required" | "optional";
}

/** 内容详情（与后端 `ContentDetail` 同构）。 */
export interface ContentDetail {
  item: ContentItem;
  project_url: string | null;
  repo_url: string | null;
  authors: string[];
  files: ContentFile[];
  game_versions: string[];
}

/** lip 环境探测结果。 */
export interface LipEnv {
  lip_available: boolean;
  message: string | null;
}

/** lip 安装结果。 */
export interface LipInstallOutcome {
  success: boolean;
  package: string;
  stdout: string;
  stderr: string;
}

/** 列表：按来源 / 类型过滤 + 关键字搜索 + 分页。 */
export function contentDownloadList(
  query?: ContentListQuery,
): Promise<ContentListPage> {
  return call<ContentListPage>("content_download_list", { query });
}

/** 详情：按跨源 id（`cf:` / `lip:`）。 */
export function contentDownloadDetail(id: string): Promise<ContentDetail> {
  return call<ContentDetail>("content_download_detail", { id });
}

/** 拉取 CurseForge 项目 readme（HTML），无则返回 null。 */
export function contentDownloadReadme(id: string): Promise<string | null> {
  return call<string | null>("content_download_readme", { id });
}

/** 下载投递：CurseForge 文件直链 → 内核下载队列，返回任务 id。 */
export function contentDownloadDownload(
  id: string,
  fileId: string,
): Promise<number> {
  return call<number>("content_download_download", { id, fileId });
}

/** 探测 lip 环境。 */
export function contentDownloadLipEnv(): Promise<LipEnv> {
  return call<LipEnv>("content_download_lip_env");
}

/** 经 lip 安装 LL 模组到目标版本目录。 */
export function contentDownloadLipInstall(
  id: string,
  version: string,
  dir?: string,
): Promise<LipInstallOutcome> {
  return call<LipInstallOutcome>("content_download_lip_install", {
    id,
    version,
    dir,
  });
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