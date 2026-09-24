// cgl-libs 模块元数据仓库 API：索引状态与远端模块条目。
//
// 契约来源：docs/cgl-libs.md 2.3.2（条目字段表）与 2.9.2 / 2.9.6（缓存与降级）。
// 内核命令（`src-tauri/src/commands/registry.rs`）：
// - `registry_status()`                      → 索引状态快照
// - `registry_refresh(force: boolean)`       → 强制 / 常规刷新，返回刷新后状态
// - `registry_modules(locale: string)`       → 远端条目数组（按 locale 取内联本地化文本）
//
// 说明：仓库侧的 schema 演进规则是「只增不改 + 客户端忽略未知字段」
// （cgl-libs.md 3.1），因此这里除安全关键字段外一律声明为可选，
// 内核侧尚未合并的字段缺失时前端不会崩。

import { call } from "./core";

/** 平台枚举（权威枚举，cgl-libs.md 2.3.6「三、平台与资产」）。 */
export type RegistryPlatform =
  | "windows-x86_64"
  | "windows-aarch64"
  | "android-arm64"
  | "linux-x86_64";

/** 发布通道（2.3.2 `channel`）。 */
export type RegistryChannel = "stable" | "beta" | "dev";

/** 收录审核状态（2.3.2 `status`）；只有 `approved` 会进 `index.json`。 */
export type RegistryEntryStatus = "approved" | "pending" | "rejected";

/**
 * 细粒度权限枚举（2.3.6「四、权限」，权威 9 项）。
 * 客户端遇到未知值必须忽略而非报错，故保留 `string` 兜底。
 */
export type RegistryPermission =
  | "filesystem:read"
  | "filesystem:write"
  | "filesystem:game-dir"
  | "network"
  | "process:spawn"
  | "download:enqueue"
  | "settings:write"
  | "intents:request"
  | "account:read";

/** 权限派生摘要（三布尔，CI 生成的只读派生值）。 */
export interface RegistryPermissionsDerived {
  network: boolean;
  spawn_process: boolean;
  write_outside_module_dir: boolean;
}

/** 条目作者信息（2.3.2 `author.*`）。 */
export interface RegistryAuthor {
  name: string;
  /** 可追责联系地址（主页或邮箱）。 */
  contact?: string;
  /** 官方身份验证；由 cgl-libs 维护者审核后置位。 */
  verified?: boolean;
}

/** 条目图标元数据（2.3.2 `icon.*`）。 */
export interface RegistryIcon {
  /** 仓库内相对路径，形如 `assets/icons/<id>.png`。 */
  path?: string;
  sha256?: string;
  /** 字节数，<= 32768。 */
  size?: number;
  /**
   * 本地缓存绝对路径（内核下载落 `<cache_dir>/registry/icons/` 后回填）。
   * 内核侧尚未提供时保持缺省，UI 回退 lucide 占位图标。
   */
  local_path?: string | null;
}

/** 下载资产（2.3.2 `assets[]`）。 */
export interface RegistryAsset {
  platform: RegistryPlatform | string;
  url: string;
  mirrors?: string[];
  size?: number;
  sha256?: string;
}

/** 一条 cgl-libs 远端模块条目（2.3.2 字段表）。 */
export interface RegistryModuleEntry {
  /** 全局唯一身份标识，两段式 `author.module`（含 `.`，**不可**用作 i18n 键前缀）。 */
  id: string;
  /** 语言包命名空间，单段无点（2.3.2 明确要求用它而非 `id` 作 i18n 前缀）。 */
  i18n_namespace?: string;
  /** 展示名；取自 manifest，可为中文。 */
  display_name?: string;
  /** 本地化简介（locale map）。权威形态，非 i18n 键。 */
  summary?: Record<string, string>;
  author?: RegistryAuthor;
  /** 源码仓库地址（manifest `homepage`）。 */
  repo?: string;
  /** SPDX 标识，闭源为 `proprietary`。 */
  license?: string;
  channel?: RegistryChannel | string;
  /** 当前条目版本，严格 semver。 */
  version?: string;
  /** 该版本发布时间（RFC3339）。 */
  published_at?: string;
  platforms?: (RegistryPlatform | string)[];
  /** 最低启动器版本（semver，扁平字段）。 */
  min_launcher?: string;
  /** 最高启动器版本（含）；`null` 表示无上限。 */
  max_launcher?: string | null;
  /** 模块 API 契约版本，当前为 `1`。 */
  api_version?: number;
  icon?: RegistryIcon;
  assets?: RegistryAsset[];
  /** 细粒度权限枚举（权威形态）。 */
  permissions?: (RegistryPermission | string)[];
  /** 权限派生摘要（只读）。 */
  permissions_derived?: RegistryPermissionsDerived;
  /** 本地化更新说明（locale map）。与 `summary` 同理，内联文本。 */
  changelog?: Record<string, string>;
  status?: RegistryEntryStatus | string;
  yanked?: boolean;
  yank_reason?: string | null;
}

/** 索引状态快照（`registry_status` / `registry_refresh` 返回值）。 */
export interface RegistryStatus {
  /** 元数据是否可用（本地缓存或远端任一成功）。 */
  available: boolean;
  /** 数据是否已超出 TTL（陈旧，UI 需提示"可能过期"）。 */
  stale: boolean;
  /** 本地缓存最近一次成功刷新的时间（RFC3339）；无缓存时为 `null`。 */
  last_updated: string | null;
  /** 索引声明的最高 `generated_at`（防降级锚点）。 */
  highest_generated_at: string | null;
  /** 索引 schema 版本；高于客户端支持值时内核拒绝解析。 */
  schema_version: number | null;
  /**
   * minisign 验签是否通过。
   * 注意：minisign 密钥尚未生成（cgl-libs.md G13），内核当前恒返回 `false`，
   * UI 必须如实呈现"签名校验未启用"，不得显示为已验签。
   */
  signature_verified: boolean;
  /** 是否正在使用 last-known-good 快照（降级状态，UI 必须可见）。 */
  using_last_known_good: boolean;
  /** 失败原因（降级 / 不可用时给出人话解释）；正常时为 `null`。 */
  error: string | null;
}

/** 索引状态快照。 */
export function registryStatus(): Promise<RegistryStatus> {
  return call<RegistryStatus>("registry_status");
}

/** 刷新索引；`force` 为 `true` 时绕过 TTL（仍受防降级校验约束）。 */
export function registryRefresh(force: boolean): Promise<RegistryStatus> {
  return call<RegistryStatus>("registry_refresh", { force });
}

/**
 * 远端模块条目列表。
 * @param locale 当前语言，供内核按 locale → en-US 回退链裁剪内联本地化文本。
 */
export function registryModules(locale: string): Promise<RegistryModuleEntry[]> {
  return call<RegistryModuleEntry[]>("registry_modules", { locale });
}

/** 不支持任何元数据时的兜底状态（内核命令不可用 / 尚未实现时使用）。 */
export function unavailableRegistryStatus(error: string): RegistryStatus {
  return {
    available: false,
    stale: false,
    last_updated: null,
    highest_generated_at: null,
    schema_version: null,
    signature_verified: false,
    using_last_known_good: false,
    error,
  };
}

/**
 * 取条目内联本地化文本（`summary` / `changelog`）。
 *
 * 回退链与内核 `I18nService::catalog` 一致：目标 locale → en-US → 空串。
 * 注意这里**不查语言包**——第三方模块的文案无法预先进入内核语言包
 * （cgl-libs.md 2.3.6 明确否决"缩略键 + 启动器自带词表"方案）。
 */
export function pickLocalized(
  text: Record<string, string> | undefined,
  locale: string,
): string {
  if (!text) return "";
  const exact = text[locale];
  if (typeof exact === "string" && exact.length > 0) return exact;
  const fallback = text["en-US"];
  if (typeof fallback === "string" && fallback.length > 0) return fallback;
  return "";
}
