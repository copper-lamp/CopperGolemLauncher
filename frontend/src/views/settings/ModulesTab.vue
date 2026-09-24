<script setup lang="ts">
// 模块 Tab：本地已注册模块 × cgl-libs 远端元数据条目的关联展示。
//
// 对应 docs/cgl-libs.md 2.9.4（展示）与 2.9.6（离线回退）：
// - 列表数据源 = `modules_list()` 左外连接 `registry_modules(locale)`（按 id）。
// - 展示名 / 简介取条目 `display_name` / `summary[locale]`，回退 en-US，再回退 id。
// - 加载中 / 离线 / 陈旧（last-known-good）必须有可见 UI，不得用空列表代表了事。
// - 框架图标一律用 lucide，不使用 emoji。

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import {
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  LoaderCircle,
  Package,
  PackageCheck,
  RefreshCw,
  ShieldAlert,
} from "@lucide/vue";

import SettingSection from "./SettingSection.vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoSwitch from "../../components/ui/CoSwitch.vue";
import { modulesSetEnabled } from "../../api/modules";
import { kernelInfo } from "../../api/theme";
import type { RegistryPermission } from "../../api/registry";
import {
  useModuleRegistry,
  type BlockReason,
  type ModuleRowView,
} from "../../composables/useModuleRegistry";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";

const { t, locale } = useI18n();

const {
  installedRows,
  installableRows,
  degraded,
  status,
  phase,
  localLoading,
  refreshing,
  ensureLoaded,
  refresh,
  reloadForLocale,
  setLauncherVersion,
} = useModuleRegistry();

/** 展开权限明细的行 id 集合。 */
const expandedPermissions = ref<Record<string, boolean>>({});
/** 本地启用状态覆盖（开关点击后的即时反馈）。 */
const enabledOverride = ref<Record<string, boolean>>({});

const M = "settings.modules";

/** 初始化：注入启动器版本供兼容性判定，并加载两侧数据。 */
onMounted(async () => {
  try {
    const info = await kernelInfo();
    setLauncherVersion(info.version);
  } catch {
    // 内核未就绪时不判负：仅跳过启动器版本相关的前置条件检查。
  }
  await ensureLoaded();
});

// 语言切换后远端条目需按新 locale 重取内联本地化文本。
watch(locale, () => void reloadForLocale());

onUnmounted(() => {
  // 单例状态刻意保留（Tab 切换不应重复拉取元数据），此处无需清理。
});

/** 行的启用状态（本地覆盖优先）。 */
function isEnabled(row: ModuleRowView): boolean {
  const override = enabledOverride.value[row.id];
  if (override !== undefined) return override;
  return row.local?.enabled ?? false;
}

/** 开关是否可用：仅已安装模块可启停。 */
function canToggle(row: ModuleRowView): boolean {
  return row.local !== null;
}

async function toggleEnabled(row: ModuleRowView, enabled: boolean) {
  const local = row.local;
  if (!local) return;
  const previous = isEnabled(row);
  enabledOverride.value = { ...enabledOverride.value, [row.id]: enabled };
  try {
    await modulesSetEnabled(local.id, enabled);
  } catch (e) {
    // 失败回滚，避免 UI 与内核状态不一致。
    enabledOverride.value = { ...enabledOverride.value, [row.id]: previous };
    showToast(String(e), "error");
  }
}

/** 行级状态徽标。 */
interface RowBadge {
  kind: string;
  label: string;
  tone: "accent" | "muted" | "warning" | "danger" | "success";
}

function badgesOf(row: ModuleRowView): RowBadge[] {
  const badges: RowBadge[] = [];
  if (row.yanked) {
    badges.push({ kind: "yanked", label: t(`${M}.status.yanked`), tone: "danger" });
  }
  if (row.hasUpdate) {
    badges.push({
      kind: "update",
      label: t(`${M}.status.update_available`),
      tone: "accent",
    });
  }
  if (!row.installed) {
    badges.push({ kind: "uninstalled", label: t(`${M}.status.not_installed`), tone: "muted" });
  }
  if (row.blockedBy.length > 0) {
    badges.push({ kind: "incompatible", label: t(`${M}.status.incompatible`), tone: "warning" });
  }
  const state = row.local?.state;
  if (state === "running") {
    badges.push({ kind: "running", label: t(`${M}.status.running`), tone: "success" });
  } else if (state === "failed") {
    badges.push({ kind: "failed", label: t(`${M}.status.failed`), tone: "danger" });
  }
  if (row.local?.suspended) {
    badges.push({ kind: "suspended", label: t(`${M}.status.suspended`), tone: "warning" });
  }
  if (row.local && row.entry === null) {
    badges.push({ kind: "local-only", label: t(`${M}.local_only`), tone: "muted" });
  }
  return badges;
}

/** 单条不可安装原因的文案。 */
function reasonText(reason: BlockReason, row: ModuleRowView): string {
  switch (reason) {
    case "yanked":
      return row.yankReason
        ? `${t(`${M}.reason.yanked`)} ${row.yankReason}`
        : t(`${M}.reason.yanked`);
    case "platform":
      return t(`${M}.reason.platform`);
    case "min_launcher":
      return t(`${M}.reason.min_launcher`, { version: row.entry?.min_launcher ?? "" });
    case "max_launcher":
      return t(`${M}.reason.max_launcher`, { version: row.entry?.max_launcher ?? "" });
    case "api_version":
      return t(`${M}.reason.api_version`);
    case "schema":
      return t(`${M}.reason.schema`);
    default:
      return "";
  }
}

function reasonsOf(row: ModuleRowView): string[] {
  return row.blockedBy.map((r) => reasonText(r, row)).filter((s) => s.length > 0);
}

/**
 * 权限细粒度枚举的框架文案键。
 * 枚举值含 `:`（如 `filesystem:game-dir`），与派生摘要的同名布尔（`network`）分处
 * 不同子命名空间，避免 JSON 键冲突；未知枚举值按 3.1「枚举扩展」规则标注而非报错。
 */
function permissionLabel(permission: RegistryPermission | string): string {
  const key = `${M}.permissions.items.${permission}`;
  const text = t(key);
  return text === key ? t(`${M}.permissions.unknown_item`, { name: permission }) : text;
}

/** 派生摘要三布尔的文案。 */
function derivedLabel(kind: "network" | "spawn_process" | "write_outside_module_dir"): string {
  return t(`${M}.permissions.derived.${kind}`);
}

function togglePermissionDetails(row: ModuleRowView) {
  expandedPermissions.value = {
    ...expandedPermissions.value,
    [row.id]: !expandedPermissions.value[row.id],
  };
}

function isPermissionExpanded(row: ModuleRowView): boolean {
  return expandedPermissions.value[row.id] === true;
}

/** 是否有权限内容需要展示（派生摘要命中或存在细粒度清单）。 */
function hasPermissionInfo(row: ModuleRowView): boolean {
  const derived = row.permissionsDerived;
  const derivedHit = Boolean(
    derived && (derived.network || derived.spawn_process || derived.write_outside_module_dir),
  );
  return derivedHit || row.permissions.length > 0;
}

/** 格式化时间戳（RFC3339 → 本地时间）；无法解析时回落原文。 */
function formatTime(value: string | null): string {
  if (!value) return t(`${M}.metadata_never_updated`);
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  return parsed.toLocaleString(locale.value);
}

/** 是否显示降级横幅。 */
const showDegradedBanner = computed(() => degraded.value && phase.value !== "loading");

/** 内核给出的失败原因。 */
const statusError = computed(() => status.value?.error ?? "");

/** 请求中的统一标志（刷新或首次加载）。 */
const busy = computed(() => refreshing.value || phase.value === "loading");

/** 可安装区块的空态文案：区分「确实没有」与「元数据未就绪」。 */
const availableEmptyText = computed(() =>
  phase.value === "unavailable"
    ? t(`${M}.metadata_unavailable_hint`)
    : t(`${M}.no_available`),
);
</script>

<template>
  <div class="modules-tab">
    <!-- 元数据状态条：加载中 / 降级 / 陈旧必须可见，不得以空列表代替 -->
    <div v-if="phase === 'loading'" class="modules-tab__banner modules-tab__banner--info">
      <LoaderCircle :size="15" class="spin" />
      <span class="modules-tab__banner-text">{{ t(`${M}.loading_metadata`) }}</span>
    </div>

    <div
      v-else-if="showDegradedBanner"
      class="modules-tab__banner modules-tab__banner--warning"
    >
      <AlertTriangle :size="15" class="modules-tab__banner-icon" />
      <div class="modules-tab__banner-text">
        <template v-if="!status?.available">
          <strong>{{ t(`${M}.metadata_unavailable_title`) }}</strong>
          <div class="modules-tab__banner-hint">
            {{ t(`${M}.metadata_unavailable_hint`) }}
            <span v-if="statusError" class="modules-tab__banner-error">{{ statusError }}</span>
          </div>
        </template>
        <template v-else>
          <strong>{{ t(`${M}.metadata_stale`) }}</strong>
          <div class="modules-tab__banner-hint">
            {{ t(`${M}.metadata_stale_hint`, { time: formatTime(status?.last_updated ?? null) }) }}
            <span v-if="status?.using_last_known_good">
              {{ t(`${M}.last_known_good`) }}
            </span>
          </div>
        </template>
      </div>
    </div>

    <!-- 签名状态：如实呈现未启用，不谎报已验签 -->
    <div
      v-if="status && !status.signature_verified"
      class="modules-tab__banner modules-tab__banner--muted"
    >
      <ShieldAlert :size="15" class="modules-tab__banner-icon" />
      <div class="modules-tab__banner-text">
        <strong>{{ t(`${M}.signature_not_enabled`) }}</strong>
        <div class="modules-tab__banner-hint">
          {{ t(`${M}.signature_not_enabled_hint`) }}
        </div>
      </div>
    </div>

    <SettingSection :title-key="`${M}.installed_section`">
      <div class="modules-tab__toolbar">
        <CoButton size="sm" :disabled="busy" @click="refresh">
          <RefreshCw :size="14" :class="{ spin: refreshing }" />
          {{ refreshing ? t(`${M}.refreshing`) : t(`${M}.refresh`) }}
        </CoButton>
      </div>

      <div v-if="installedRows.length === 0" class="modules-tab__empty">
        {{ t(`${M}.no_installed`) }}
      </div>

      <div v-for="row in installedRows" :key="row.id" class="modules-tab__row">
        <div class="modules-tab__main">
          <!-- 图标：有本地缓存则显示，否则回退 lucide 占位 -->
          <img
            v-if="row.iconPath"
            class="modules-tab__icon"
            :src="row.iconPath"
            :alt="row.displayName"
          />
          <Package v-else :size="26" class="modules-tab__icon modules-tab__icon--placeholder" />

          <div class="modules-tab__info">
            <div class="modules-tab__title-line">
              <span class="modules-tab__name">{{ row.displayName }}</span>
              <span
                v-for="badge in badgesOf(row)"
                :key="badge.kind"
                :class="['modules-tab__badge', `modules-tab__badge--${badge.tone}`]"
              >
                {{ badge.label }}
              </span>
              <span v-if="row.builtin" class="modules-tab__badge modules-tab__badge--muted">
                {{ t(`${M}.builtin`) }}
              </span>
            </div>

            <div v-if="row.summary" class="modules-tab__summary">{{ row.summary }}</div>

            <div class="modules-tab__meta">
              <span class="modules-tab__meta-item">{{ row.id }}</span>
              <span v-if="row.installedVersion" class="modules-tab__meta-item">
                {{ t(`${M}.version.installed`, { version: row.installedVersion }) }}
              </span>
              <span v-else class="modules-tab__meta-item modules-tab__meta-item--muted">
                {{ t(`${M}.version.unknown`) }}
              </span>
              <span
                v-if="row.hasUpdate && row.remoteVersion"
                class="modules-tab__meta-item modules-tab__meta-item--accent"
              >
                {{ t(`${M}.version.remote`, { version: row.remoteVersion }) }}
              </span>
            </div>

            <ul v-if="reasonsOf(row).length > 0" class="modules-tab__reasons">
              <li v-for="(reason, idx) in reasonsOf(row)" :key="idx">{{ reason }}</li>
            </ul>

            <div v-else-if="row.local && row.entry === null" class="modules-tab__local-only">
              <CircleAlert :size="13" />
              <span>{{ t(`${M}.local_only_hint`) }}</span>
            </div>

            <div v-if="row.local?.error" class="modules-tab__error">{{ row.local.error }}</div>

            <!-- 权限横幅：三布尔摘要，可展开细粒度清单 -->
            <div v-if="hasPermissionInfo(row)" class="modules-tab__perms">
              <button class="modules-tab__perms-toggle" @click="togglePermissionDetails(row)">
                <component
                  :is="isPermissionExpanded(row) ? ChevronDown : ChevronRight"
                  :size="13"
                />
                <span>{{ t(`${M}.permissions.summary`) }}</span>
              </button>
              <span class="modules-tab__perms-summary">
                <template v-if="row.permissionsDerived">
                  <span v-if="row.permissionsDerived.network" class="modules-tab__chip">
                    {{ derivedLabel("network") }}
                  </span>
                  <span v-if="row.permissionsDerived.spawn_process" class="modules-tab__chip">
                    {{ derivedLabel("spawn_process") }}
                  </span>
                  <span
                    v-if="row.permissionsDerived.write_outside_module_dir"
                    class="modules-tab__chip"
                  >
                    {{ derivedLabel("write_outside_module_dir") }}
                  </span>
                </template>
                <span
                  v-if="row.permissions.length === 0 || !row.permissionsDerived"
                  class="modules-tab__chip modules-tab__chip--muted"
                >
                  {{ t(`${M}.permissions.none`) }}
                </span>
              </span>
              <ul v-if="isPermissionExpanded(row)" class="modules-tab__perms-list">
                <li v-for="permission in row.permissions" :key="permission">
                  {{ permissionLabel(permission) }}
                  <code class="modules-tab__perm-code">{{ permission }}</code>
                </li>
              </ul>
            </div>
          </div>

          <div class="modules-tab__control">
            <CoSwitch
              :model-value="isEnabled(row)"
              :disabled="!canToggle(row)"
              :label="row.displayName"
              @update:model-value="toggleEnabled(row, $event)"
            />
          </div>
        </div>
      </div>
    </SettingSection>

    <SettingSection :title-key="`${M}.available_section`">
      <div v-if="phase === 'loading' || localLoading" class="modules-tab__empty">
        {{ t(`${M}.loading_metadata`) }}
      </div>
      <div v-else-if="installableRows.length === 0" class="modules-tab__empty">
        {{ availableEmptyText }}
      </div>

      <div v-for="row in installableRows" :key="row.id" class="modules-tab__row">
        <div class="modules-tab__main">
          <img
            v-if="row.iconPath"
            class="modules-tab__icon"
            :src="row.iconPath"
            :alt="row.displayName"
          />
          <Package v-else :size="26" class="modules-tab__icon modules-tab__icon--placeholder" />

          <div class="modules-tab__info">
            <div class="modules-tab__title-line">
              <span class="modules-tab__name">{{ row.displayName }}</span>
              <span
                v-for="badge in badgesOf(row)"
                :key="badge.kind"
                :class="['modules-tab__badge', `modules-tab__badge--${badge.tone}`]"
              >
                {{ badge.label }}
              </span>
            </div>

            <div v-if="row.summary" class="modules-tab__summary">{{ row.summary }}</div>

            <div class="modules-tab__meta">
              <span class="modules-tab__meta-item">{{ row.id }}</span>
              <span v-if="row.remoteVersion" class="modules-tab__meta-item">
                {{ t(`${M}.version.remote`, { version: row.remoteVersion }) }}
              </span>
              <span
                v-if="row.entry?.author?.verified"
                class="modules-tab__meta-item modules-tab__meta-item--accent"
              >
                {{ t(`${M}.author.verified`) }}
              </span>
              <span v-if="row.entry?.author?.name" class="modules-tab__meta-item">
                {{ t(`${M}.author.by`, { name: row.entry.author.name }) }}
              </span>
            </div>

            <ul v-if="reasonsOf(row).length > 0" class="modules-tab__reasons">
              <li v-for="(reason, idx) in reasonsOf(row)" :key="idx">{{ reason }}</li>
            </ul>

            <div v-if="hasPermissionInfo(row)" class="modules-tab__perms">
              <button class="modules-tab__perms-toggle" @click="togglePermissionDetails(row)">
                <component
                  :is="isPermissionExpanded(row) ? ChevronDown : ChevronRight"
                  :size="13"
                />
                <span>{{ t(`${M}.permissions.summary`) }}</span>
              </button>
              <span class="modules-tab__perms-summary">
                <template v-if="row.permissionsDerived">
                  <span v-if="row.permissionsDerived.network" class="modules-tab__chip">
                    {{ derivedLabel("network") }}
                  </span>
                  <span v-if="row.permissionsDerived.spawn_process" class="modules-tab__chip">
                    {{ derivedLabel("spawn_process") }}
                  </span>
                  <span
                    v-if="row.permissionsDerived.write_outside_module_dir"
                    class="modules-tab__chip"
                  >
                    {{ derivedLabel("write_outside_module_dir") }}
                  </span>
                </template>
                <span
                  v-if="row.permissions.length === 0 || !row.permissionsDerived"
                  class="modules-tab__chip modules-tab__chip--muted"
                >
                  {{ t(`${M}.permissions.none`) }}
                </span>
              </span>
              <ul v-if="isPermissionExpanded(row)" class="modules-tab__perms-list">
                <li v-for="permission in row.permissions" :key="permission">
                  {{ permissionLabel(permission) }}
                  <code class="modules-tab__perm-code">{{ permission }}</code>
                </li>
              </ul>
            </div>
          </div>

          <div class="modules-tab__control">
            <!--
              安装动作（差距项 G4）的内核命令 modules_install 尚未合并：
              按钮置灰并给出原因，不做假实现。
            -->
            <CoButton
              size="sm"
              variant="primary"
              disabled
              :title="reasonsOf(row)[0] ?? t(`${M}.actions.not_supported`)"
            >
              <PackageCheck :size="14" />
              {{ t(`${M}.actions.install`) }}
            </CoButton>
          </div>
        </div>
      </div>
    </SettingSection>
  </div>
</template>

<style scoped>
.modules-tab {
  max-width: 760px;
}

.modules-tab__banner {
  display: flex;
  align-items: flex-start;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3);
  margin-bottom: var(--copper-space-4);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  font-size: var(--copper-font-size-sm);
}

.modules-tab__banner--info {
  background: var(--copper-surface);
  color: var(--copper-text-secondary);
}

.modules-tab__banner--warning {
  background: color-mix(in srgb, var(--copper-warning) 10%, var(--copper-surface));
  border-color: color-mix(in srgb, var(--copper-warning) 40%, var(--copper-border));
  color: var(--copper-warning);
}

.modules-tab__banner--muted {
  background: var(--copper-surface);
  color: var(--copper-text-secondary);
}

.modules-tab__banner-icon {
  flex-shrink: 0;
  margin-top: 1px;
}

.modules-tab__banner-text {
  min-width: 0;
}

.modules-tab__banner-hint {
  margin-top: 2px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.modules-tab__banner-error {
  display: block;
  color: var(--copper-danger);
  overflow-wrap: anywhere;
}

.modules-tab__toolbar {
  display: flex;
  justify-content: flex-end;
  padding: var(--copper-space-2) 0;
}

.modules-tab__empty {
  padding: var(--copper-space-6) 0;
  text-align: center;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.modules-tab__row {
  border-top: 1px solid var(--copper-border);
}

.modules-tab__main {
  display: flex;
  align-items: flex-start;
  gap: var(--copper-space-3);
  min-height: 52px;
  padding: var(--copper-space-3) 0;
}

.modules-tab__icon {
  flex-shrink: 0;
  width: 32px;
  height: 32px;
  border-radius: var(--copper-radius-sm);
  object-fit: contain;
}

.modules-tab__icon--placeholder {
  color: var(--copper-text-secondary);
  padding: 3px;
  background: var(--copper-surface-2);
}

.modules-tab__info {
  flex: 1;
  min-width: 0;
}

.modules-tab__title-line {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--copper-space-2);
}

.modules-tab__name {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
}

.modules-tab__badge {
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  font-size: var(--copper-font-size-xs);
}

.modules-tab__badge--accent {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
  color: var(--copper-accent);
}

.modules-tab__badge--muted {
  background: var(--copper-surface-3);
  color: var(--copper-text-secondary);
}

.modules-tab__badge--warning {
  background: color-mix(in srgb, var(--copper-warning) 14%, transparent);
  color: var(--copper-warning);
}

.modules-tab__badge--danger {
  background: color-mix(in srgb, var(--copper-danger) 14%, transparent);
  color: var(--copper-danger);
}

.modules-tab__badge--success {
  background: color-mix(in srgb, var(--copper-success) 14%, transparent);
  color: var(--copper-success);
}

.modules-tab__summary {
  margin-top: 3px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  line-height: 1.5;
  overflow-wrap: anywhere;
}

.modules-tab__meta {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--copper-space-3);
  margin-top: 4px;
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-secondary);
}

.modules-tab__meta-item--muted {
  color: var(--copper-text-disabled);
}

.modules-tab__meta-item--accent {
  color: var(--copper-accent);
}

.modules-tab__reasons {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--copper-space-2);
  margin: 4px 0 0;
  padding: 0;
  list-style: none;
  color: var(--copper-warning);
  font-size: var(--copper-font-size-xs);
}

.modules-tab__reasons li + li::before {
  content: "·";
  margin-right: var(--copper-space-1);
}

.modules-tab__local-only {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
  margin-top: 4px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.modules-tab__error {
  margin-top: 4px;
  color: var(--copper-danger);
  font-size: var(--copper-font-size-xs);
  overflow-wrap: anywhere;
}

.modules-tab__perms {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--copper-space-2);
  margin-top: 6px;
}

.modules-tab__perms-toggle {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  border: none;
  background: transparent;
  padding: 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  transition: color var(--copper-duration-fast) var(--copper-easing);
}

.modules-tab__perms-toggle:hover {
  color: var(--copper-text);
}

.modules-tab__perms-summary {
  display: inline-flex;
  flex-wrap: wrap;
  gap: var(--copper-space-1);
}

.modules-tab__chip {
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  background: color-mix(in srgb, var(--copper-warning) 14%, transparent);
  color: var(--copper-warning);
  font-size: var(--copper-font-size-xs);
}

.modules-tab__chip--muted {
  background: var(--copper-surface-3);
  color: var(--copper-text-secondary);
}

.modules-tab__perms-list {
  flex-basis: 100%;
  margin: var(--copper-space-1) 0 0;
  padding-left: var(--copper-space-5);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  line-height: 1.7;
}

.modules-tab__perm-code {
  margin-left: var(--copper-space-2);
  color: var(--copper-text-disabled);
}

.modules-tab__control {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  padding-top: 4px;
}

.spin {
  animation: spin 1.2s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
