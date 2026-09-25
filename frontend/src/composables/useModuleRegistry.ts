// 模块 Tab 状态：本地已注册模块 × cgl-libs 远端元数据条目的左外连接视图。
//
// 分工（docs/cgl-libs.md 2.9.4）：
// - 本地侧来自 `modules_list()`，是唯一权威的"这台机器装了什么"。
// - 远端侧来自 `registry_modules(locale)`，提供展示名 / 简介 / 版本 / 兼容区间 / 权限。
// - 两侧按 `id` 关联；远端有而本地无 → "未安装（可安装）"；本地有而远端无 → 仅本地信息。
//
// 本模块不做任何网络或缓存实现，只消费内核命令并派生 UI 状态。

import { computed, ref } from "vue";

import {
  modulesList,
  type ModuleInfo,
} from "../api/modules";
import {
  pickLocalized,
  registryModules,
  registryRefresh,
  registryStatus,
  unavailableRegistryStatus,
  type RegistryModuleEntry,
  type RegistryStatus,
} from "../api/registry";
import { useLocaleRef } from "../i18n";
import { currentPlatform } from "./usePlatform";

/** 内核 `Module` trait 契约版本，当前为 1（2.3.2 `api_version`）。 */
const SUPPORTED_API_VERSION = 1;

/**
 * 客户端支持的元数据契约版本（cgl-libs.md 2.5 规则 4 / 3.1）。
 *
 * 索引 `schema_version` 高于该值时内核会硬拒绝整份索引，前端据此在横幅上
 * 提示"需要升级启动器"，而不是把该情况降级成单行不兼容原因。
 */
const SUPPORTED_SCHEMA_VERSION = 1;

/**
 * 解析 semver 为可比较的数值三元组；非法输入返回 `null`。
 */
function parseSemver(value: string | null | undefined): number[] | null {
  if (typeof value !== "string") return null;
  const core = value.trim().replace(/^v/i, "").split(/[-+]/)[0];
  const parts = core.split(".");
  if (parts.length === 0) return null;
  const nums: number[] = [];
  for (const part of parts) {
    if (!/^\d+$/.test(part)) return null;
    nums.push(Number(part));
  }
  while (nums.length < 3) nums.push(0);
  return nums;
}

/** 比较两个 semver：`a > b` 返回正数，`a < b` 返回负数，相等返回 0；非法输入返回 `null`。 */
export function compareSemver(a: string, b: string): number | null {
  const left = parseSemver(a);
  const right = parseSemver(b);
  if (!left || !right) return null;
  const len = Math.max(left.length, right.length);
  for (let i = 0; i < len; i += 1) {
    const l = left[i] ?? 0;
    const r = right[i] ?? 0;
    if (l !== r) return l - r;
  }
  return 0;
}

/** 条目为何不可安装（空数组表示可安装）。 */
export type BlockReason =
  | "yanked"
  | "platform"
  | "min_launcher"
  | "max_launcher"
  | "api_version"
  | "schema";

/** 单个模块行的派生视图。 */
export interface ModuleRowView {
  id: string;
  /** 本地已注册模块；远端独有（未安装）时为 `null`。 */
  local: ModuleInfo | null;
  /** 远端元数据条目；本地独有（未收录 / 离线）时为 `null`。 */
  entry: RegistryModuleEntry | null;
  /** 展示名：本地 display_name → 条目 display_name → id。 */
  displayName: string;
  /** 简介（内联本地化文本，不走语言包）；无元数据时为空串。 */
  summary: string;
  /** 本地已装版本。 */
  installedVersion: string | null;
  /** 远端最新版本。 */
  remoteVersion: string | null;
  /** 是否有可用更新（两侧版本均可比较且远端更大）。 */
  hasUpdate: boolean;
  /** 是否已安装（本地已注册）。 */
  installed: boolean;
  /** 是否内置模块。 */
  builtin: boolean;
  /** 是否已 yank（仍可见但禁止新安装）。 */
  yanked: boolean;
  yankReason: string;
  /** 全部不满足的安装前置条件；已安装的模块不阻塞启停，仅作提示。 */
  blockedBy: BlockReason[];
  /** 展示用的图标本地缓存路径；缺省则回退 lucide 占位图标。 */
  iconPath: string | null;
  /** 权限派生摘要；条目缺省时为 `null`。 */
  permissionsDerived: RegistryModuleEntry["permissions_derived"] | null;
  /** 细粒度权限枚举清单。 */
  permissions: string[];
}

/** 元数据就绪状态（决定列表如何降级展示）。 */
export type MetadataPhase = "idle" | "loading" | "ready" | "unavailable";

/**
 * 模块注册表状态。作为**模块级单例**：设置页 Tab 切换会卸载组件，
 * 但元数据不应因此重复拉取；刷新动作也需在重新挂载后保持结果。
 */
const localModules = ref<ModuleInfo[]>([]);
const entries = ref<RegistryModuleEntry[]>([]);
const status = ref<RegistryStatus | null>(null);
const phase = ref<MetadataPhase>("idle");
const localLoading = ref(false);
const refreshing = ref(false);

const locale = useLocaleRef();

let inflight: Promise<void> | null = null;

/** 展示名回退链：本地 display_name → 条目 display_name → id（2.9.4）。 */
function resolveDisplayName(local: ModuleInfo | null, entry: RegistryModuleEntry | null): string {
  const localName = local?.display_name?.trim();
  if (localName) return localName;
  const entryName = entry?.display_name?.trim();
  if (entryName) return entryName;
  return local?.id ?? entry?.id ?? "";
}

/**
 * 计算条目相对当前启动器版本的不兼容原因。
 *
 * `schemaVersion` 为索引整体声明的契约版本：高于客户端支持值时，所有条目都
 * 视为不可用（内核不会返回条目，此处是可测的纯逻辑兜底）。
 */
function entryBlockedBy(
  entry: RegistryModuleEntry,
  launcherVersion: string | null,
  schemaVersion: number | null,
): BlockReason[] {
  const reasons: BlockReason[] = [];

  if (entry.yanked === true) reasons.push("yanked");

  if (typeof schemaVersion === "number" && schemaVersion > SUPPORTED_SCHEMA_VERSION) {
    reasons.push("schema");
  }

  // 平台不兼容：`platforms` 缺省时不判负（容忍内核尚未合并该字段）。
  if (Array.isArray(entry.platforms) && entry.platforms.length > 0) {
    if (!entry.platforms.includes(currentPlatform())) reasons.push("platform");
  }

  if (launcherVersion) {
    if (entry.min_launcher) {
      const cmp = compareSemver(launcherVersion, entry.min_launcher);
      if (cmp !== null && cmp < 0) reasons.push("min_launcher");
    }
    if (typeof entry.max_launcher === "string" && entry.max_launcher.length > 0) {
      const cmp = compareSemver(launcherVersion, entry.max_launcher);
      if (cmp !== null && cmp > 0) reasons.push("max_launcher");
    }
  }

  // `api_version` 高于内核契约版本时该模块无法被内核装载。
  if (typeof entry.api_version === "number" && entry.api_version > SUPPORTED_API_VERSION) {
    reasons.push("api_version");
  }

  return reasons;
}

/**
 * 索引 schema 版本是否高于客户端支持值（cgl-libs.md 3.1：客户端必须拒绝）。
 * 该情况下内核不会返回条目，属于整体降级而非单行问题，故在横幅上提示。
 */
function computeSchemaTooNew(s: RegistryStatus | null): boolean {
  const version = s?.schema_version;
  return typeof version === "number" && version > SUPPORTED_SCHEMA_VERSION;
}

/** 左外连接并派生展示视图。 */
function buildRows(
  locals: ModuleInfo[],
  remote: RegistryModuleEntry[],
  launcherVersion: string | null,
  schemaVersion: number | null,
): ModuleRowView[] {
  const entriesById = new Map<string, RegistryModuleEntry>();
  for (const entry of remote) {
    if (typeof entry?.id === "string" && entry.id.length > 0) entriesById.set(entry.id, entry);
  }

  const rows: ModuleRowView[] = [];
  const seen = new Set<string>();

  // 先本地模块（保持内核返回顺序），携带远端条目。
  for (const local of locals) {
    seen.add(local.id);
    const entry = entriesById.get(local.id) ?? null;
    rows.push(makeRow(local, entry, launcherVersion, schemaVersion));
  }

  // 再追加远端独有（未安装）条目，按展示名稳定排序方便查找。
  const uninstalled: ModuleRowView[] = [];
  for (const entry of remote) {
    if (typeof entry?.id !== "string" || seen.has(entry.id)) continue;
    uninstalled.push(makeRow(null, entry, launcherVersion, schemaVersion));
  }
  uninstalled.sort((a, b) => a.displayName.localeCompare(b.displayName));
  rows.push(...uninstalled);

  return rows;
}

function makeRow(
  local: ModuleInfo | null,
  entry: RegistryModuleEntry | null,
  launcherVersion: string | null,
  schemaVersion: number | null,
): ModuleRowView {
  const installedVersion = local?.version ?? null;
  const remoteVersion = entry?.version ?? null;

  let hasUpdate = false;
  if (installedVersion && remoteVersion) {
    const cmp = compareSemver(remoteVersion, installedVersion);
    hasUpdate = cmp !== null && cmp > 0;
  }

  return {
    id: local?.id ?? entry?.id ?? "",
    local,
    entry,
    displayName: resolveDisplayName(local, entry),
    summary: pickLocalized(entry?.summary, locale.value),
    installedVersion,
    remoteVersion,
    hasUpdate,
    installed: local !== null,
    builtin: local !== null ? local.is_builtin : false,
    yanked: entry?.yanked === true,
    yankReason: entry?.yank_reason ?? "",
    blockedBy: entry ? entryBlockedBy(entry, launcherVersion, schemaVersion) : [],
    iconPath: entry?.icon?.local_path ?? null,
    permissionsDerived: entry?.permissions_derived ?? null,
    permissions: Array.isArray(entry?.permissions) ? entry.permissions : [],
  };
}

/** 当前启动器版本注入点（由模块 Tab 在挂载时经 `kernel_info` 写入）。 */
const launcherVersion = ref<string | null>(null);

/** 更新用于兼容性判定的启动器版本。 */
export function setLauncherVersion(version: string | null): void {
  launcherVersion.value = version;
}

/** 拉取本地模块列表。 */
async function loadLocal(): Promise<void> {
  localLoading.value = true;
  try {
    localModules.value = await modulesList();
  } finally {
    localLoading.value = false;
  }
}

/**
 * 拉取远端元数据与状态；`force` 为 `true` 时绕过内核 TTL。
 * 失败时降级为 `unavailable`，**保留**本地模块列表（2.9.6：离线仍可列出与启停已装模块）。
 */
async function loadRegistry(force: boolean): Promise<void> {
  try {
    const next = force ? await registryRefresh(true) : await registryStatus();
    status.value = next;
    if (!next.available) {
      phase.value = "unavailable";
      entries.value = [];
      return;
    }
    entries.value = await registryModules(locale.value);
    phase.value = "ready";
  } catch (e) {
    // 内核侧命令尚未实现（功能未合并）或索引不可用：如实降级，不假装正常。
    status.value = unavailableRegistryStatus(e instanceof Error ? e.message : String(e));
    entries.value = [];
    phase.value = "unavailable";
  }
}

/** 首次加载：本地与远端并行，任一侧失败不影响另一侧展示。 */
async function ensureLoaded(): Promise<void> {
  if (phase.value !== "idle") return;
  if (inflight) return inflight;
  phase.value = "loading";
  inflight = (async () => {
    await Promise.allSettled([loadLocal(), loadRegistry(false)]);
    if (phase.value === "loading") phase.value = "unavailable";
  })().finally(() => {
    inflight = null;
  });
  return inflight;
}

/** 手动刷新：本地模块列表与远端元数据一起重取。 */
async function refresh(): Promise<void> {
  if (refreshing.value) return;
  refreshing.value = true;
  try {
    await Promise.allSettled([loadLocal(), loadRegistry(true)]);
  } finally {
    refreshing.value = false;
  }
}

/** 语言切换后重新取内联本地化文本（远端条目按 locale 返回）。 */
async function reloadForLocale(): Promise<void> {
  if (phase.value !== "ready") return;
  try {
    entries.value = await registryModules(locale.value);
  } catch {
    // 切换语言失败时保留原文本，不打断界面。
  }
}

/** 可组合式入口。 */
export function useModuleRegistry() {
  const rows = computed(() =>
    buildRows(
      localModules.value,
      entries.value,
      launcherVersion.value,
      status.value?.schema_version ?? null,
    ),
  );

  /** 已安装的行。 */
  const installedRows = computed(() => rows.value.filter((r) => r.installed));
  /** 未安装但可安装的行。 */
  const installableRows = computed(() => rows.value.filter((r) => !r.installed));
  /** 列表整体是否为空（本地与远端都没有）。 */
  const isEmpty = computed(() => rows.value.length === 0);
  /** 元数据是否处于降级状态（离线 / 陈旧 / last-known-good）。 */
  const degraded = computed(() => {
    const s = status.value;
    if (!s) return false;
    return !s.available || s.stale || s.using_last_known_good;
  });
  /** 索引契约版本是否高于客户端支持值（需提示升级启动器）。 */
  const schemaTooNew = computed(() => computeSchemaTooNew(status.value));

  return {
    rows,
    installedRows,
    installableRows,
    isEmpty,
    degraded,
    schemaTooNew,
    status,
    phase,
    localLoading,
    refreshing,
    ensureLoaded,
    refresh,
    reloadForLocale,
    setLauncherVersion,
  };
}
