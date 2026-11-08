<script setup lang="ts">
// 实例入口页（路由 `/version-settings`）：实例详情 + 动作入口。
//
// 版面照 V3 稿第 9 屏：左侧是实例信息（封面 / 名称 / 游戏版本与创建日期 / 模式分段 /
// 三个开关），右侧是竖排动作按钮（管理类 + 还原 / 修改 / 删除）。左侧 6 : 右侧 4，
// 与另外三个管理页同一套分栏语言。
//
// 这一版推翻了旧「左版本标签页 + 顶部标签页」的浏览器标签咬合结构：版本切换在开始页
// 完成，实例页只描述"当前这一个实例"，不再兼任版本列表。因此入口页必须能拿到实例 ——
// 路由 `?name=` 为主，缺失时由 `useVersionInstance` 回落到上次选中 / 清单首项。
//
// 模式写入走 `homeVersionSaveMeta({ mode })`：分段控件的选中态只跟随后端返回的视图，
// 不做乐观更新，失败时不会留下"看起来切过去了"的假状态。开关同理。

import { computed, ref, watch, type Component } from "vue";
import { useRouter } from "vue-router";
import {
  Boxes,
  Gamepad2,
  MonitorSmartphone,
  PlugZap,
  Puzzle,
  RotateCcw,
  Settings2,
  Trash2,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { usePlatform } from "../../composables/usePlatform";
import CoSegmented from "../../components/ui/CoSegmented.vue";
import CoSwitch from "../../components/ui/CoSwitch.vue";
import IconEditDialog from "./IconEditDialog.vue";
import InstanceEditDialog from "./InstanceEditDialog.vue";
import {
  homeLaunchFileRestore,
  homeLaunchFileState,
  homeVersionDelete,
  homeVersionSaveMeta,
  type LaunchFileState,
  type VersionView,
} from "../../api/home";
import { useVersionInstance } from "./useVersionInstance";

/** 右侧动作按钮。稿子里的按钮只有文字，没有图标，这里保持一致。 */
interface EntryAction {
  key: string;
  icon: Component;
  label: string;
  /** 禁用原因（悬停提示）；为空表示可用。 */
  reason?: string;
  disabled?: boolean;
  danger?: boolean;
  run: () => void;
}

/** 右栏按语义分组：管理（进子页）与维护（改实例本身）。 */
interface EntryActionGroup {
  key: string;
  label: string;
  items: EntryAction[];
}

const { t } = useI18n();
const router = useRouter();
const { isMobile, isWindows } = usePlatform();
const { current, currentName, loading, refresh, applyView } = useVersionInstance();

const iconDialogOpen = ref(false);
const editDialogOpen = ref(false);
const switching = ref(false);
const saving = ref(false);
const restoring = ref(false);
const launchState = ref<LaunchFileState | null>(null);

/** 创建日期只取年月日，RFC3339 的时分秒对用户没有意义。 */
const createdLabel = computed(() => {
  const raw = current.value?.created_at ?? "";
  return raw.length >= 10 ? raw.slice(0, 10).replace(/-/g, ".") : "";
});

const typeLabel = computed(() => {
  const key = `module.home.type.${current.value?.version_type ?? ""}`;
  const label = t(key);
  return label === key ? current.value?.version_type ?? "" : label;
});

// ---------------------------------------------------------------- 模式

const modeOptions = computed(() => {
  const options = [
    { value: "original", label: t("module.home.instance.mode.original") },
    { value: "levi", label: t("module.home.instance.mode.levi") },
  ];
  // 客户端注入只有 Windows 有实现；移动端 / Linux / macOS 不提供该选项。
  // 旧数据已是 client 模式时仍显示，否则分段控件会没有选中项。
  if (isWindows.value || current.value?.mode === "client") {
    options.push({ value: "client", label: t("module.home.instance.mode.client") });
  }
  return options;
});

/** 只在"当前模式缺东西"时提示，正常状态不占用版面。 */
const modeWarning = computed(() => {
  const view = current.value;
  if (!view) return "";
  if (view.mode === "levi" && !view.loader) {
    return t("module.home.instance.mode.hint_levi_missing");
  }
  if (view.mode === "client") {
    if (!isWindows.value) return t("module.home.instance.mode.hint_client_unsupported");
    if (!view.client_id) return t("module.home.instance.mode.hint_client_unset");
  }
  return "";
});

/** 当前模式的说明：只在说明得清时占一行，缺条件的情况交给上面的警告。 */
const modeHint = computed(() => {
  const view = current.value;
  if (!view) return "";
  if (view.mode === "original") {
    return t("module.home.instance.mode.hint_original");
  }
  if (view.mode === "levi" && view.loader) {
    return t("module.home.instance.mode.hint_levi_installed", { loader: view.loader });
  }
  return "";
});

async function changeMode(value: string) {
  const view = current.value;
  if (!view || value === view.mode) return;
  switching.value = true;
  try {
    const updated = await homeVersionSaveMeta(view.name, { mode: value });
    applyView(updated);
    showToast(t("module.home.instance.mode.changed"), "success");
  } catch (e) {
    showToast(t("module.home.instance.mode.switch_failed", { message: String(e) }), "error");
  } finally {
    switching.value = false;
  }
}

// ---------------------------------------------------------------- 开关

async function saveToggle(update: {
  enable_console?: boolean;
  enable_editor_mode?: boolean;
}) {
  const view = current.value;
  if (!view) return;
  saving.value = true;
  try {
    applyView(await homeVersionSaveMeta(view.name, update));
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    saving.value = false;
  }
}

// ---------------------------------------------------------------- 启动文件状态

async function refreshLaunchState() {
  const name = current.value?.name;
  if (!name) return;
  try {
    launchState.value = await homeLaunchFileState(name);
  } catch {
    // 非 Windows / 从未注入：拿不到状态就不给「还原」假承诺。
    launchState.value = null;
  }
}

const canRestore = computed(() => !!launchState.value?.backup_exists && !restoring.value);

/** 还原按钮的禁用原因：稿子里没有说明位，用 title 承载。 */
const restoreReason = computed(() => {
  if (!launchState.value) return t("module.home.instance.action.restore_unsupported");
  if (!launchState.value.backup_exists) return t("module.home.instance.action.restore_none");
  return "";
});

async function restoreVersion() {
  const view = current.value;
  if (!view || !canRestore.value) return;
  if (!window.confirm(t("module.home.instance.action.restore_confirm", { name: view.name }))) return;
  restoring.value = true;
  try {
    await homeLaunchFileRestore(view.name);
    showToast(t("module.home.toast.launch_file_restored"), "success");
  } catch (e) {
    showToast(
      t("module.home.toast.launch_file_restore_failed", { message: String(e) }),
      "error",
    );
  } finally {
    restoring.value = false;
    await refreshLaunchState();
  }
}

// ---------------------------------------------------------------- 动作

function go(section: "content" | "mods" | "client" | "controls") {
  if (!currentName.value) return;
  void router.push({
    path: `/version-settings/${section}`,
    query: { name: currentName.value },
  });
}

/**
 * 右栏动作按语义分两组：管理（进子页）与维护（改实例本身）。
 * 分组不是为了好看：还原 / 修改 / 删除都不可逆，混在"管理资源"里点错代价很高。
 */
const actionGroups = computed<EntryActionGroup[]>(() => {
  const view = current.value;
  if (!view) return [];

  const manage: EntryAction[] = [];
  // 中间那格随模式变化：原版不显示，模组模式进模组页，客户端模式进客户端页。
  if (view.mode === "levi") {
    manage.push({
      key: "mods",
      icon: Puzzle,
      label: t("module.home.instance.action.mods"),
      run: () => go("mods"),
    });
  } else if (view.mode === "client") {
    manage.push({
      key: "client",
      icon: PlugZap,
      label: t("module.home.instance.action.client"),
      run: () => go("client"),
    });
  }
  manage.push({
    key: "content",
    icon: Boxes,
    label: t("module.home.instance.action.content"),
    run: () => go("content"),
  });
  if (isMobile.value) {
    manage.push({
      key: "controls",
      icon: MonitorSmartphone,
      label: t("module.home.instance.action.controls"),
      run: () => go("controls"),
    });
  }

  const maintain: EntryAction[] = [
    {
      key: "restore",
      icon: RotateCcw,
      label: t("module.home.instance.action.restore"),
      reason: restoreReason.value,
      disabled: !canRestore.value,
      run: () => void restoreVersion(),
    },
    {
      key: "edit",
      icon: Settings2,
      label: t("module.home.instance.action.edit"),
      run: () => (editDialogOpen.value = true),
    },
    {
      key: "delete",
      icon: Trash2,
      label: t("module.home.instance.action.delete"),
      danger: true,
      run: () => void deleteVersion(),
    },
  ];

  return [
    { key: "manage", label: t("module.home.instance.group.manage"), items: manage },
    { key: "maintain", label: t("module.home.instance.group.maintain"), items: maintain },
  ];
});

// ---------------------------------------------------------------- 生命周期

/** 当前实例变化后重取启动文件状态（还原按钮的可用性依赖它）。 */
watch(currentName, () => void refreshLaunchState(), { immediate: true });

async function deleteVersion() {
  const target = current.value;
  if (!target) return;
  if (!window.confirm(t("module.home.delete_confirm", { name: target.name }))) return;
  try {
    await homeVersionDelete(target.name);
    showToast(t("module.home.toast.deleted"), "success");
    await refresh();
    if (!current.value) void router.push("/");
  } catch (e) {
    showToast(String(e), "error");
  }
}

/** 修改实例弹窗保存成功：同步清单与路由（重命名会换名字）。 */
function onUpdated(updated: VersionView) {
  applyView(updated);
  void refreshLaunchState();
}
</script>

<template>
  <div class="entry">
    <div v-if="current" class="entry__grid">
      <section class="entry__main">
        <header class="entry__hero">
          <button
            type="button"
            class="entry__cover"
            :title="t('module.home.basic.logo_hint')"
            @click="iconDialogOpen = true"
          >
            <img v-if="current.logo_data_url" :src="current.logo_data_url" alt="" />
            <Gamepad2 v-else :size="34" :stroke-width="1.2" />
          </button>

          <div class="entry__ident">
            <h1 class="entry__name">{{ current.name }}</h1>
            <p class="entry__meta">
              <span>{{ current.game_version }}</span>
              <span v-if="createdLabel">{{ createdLabel }}</span>
              <span>{{ typeLabel }}</span>
            </p>
          </div>
        </header>

        <div class="entry__section">
          <span class="entry__section-label">{{ t("module.home.instance.mode.label") }}</span>
          <div class="entry__mode">
            <CoSegmented
              :model-value="current.mode"
              :options="modeOptions"
              :disabled="switching"
              @update:model-value="changeMode"
            />
          </div>
          <p v-if="modeWarning" class="entry__warning">{{ modeWarning }}</p>
          <p v-else-if="modeHint" class="entry__hint">{{ modeHint }}</p>
        </div>

        <div class="entry__section">
          <span class="entry__section-label">{{ t("module.home.instance.options.label") }}</span>
          <ul class="entry__rows">
            <li class="entry__row">
              <span class="entry__row-label">{{ t("module.home.meta.isolation") }}</span>
              <CoSwitch :model-value="true" disabled />
            </li>
            <li class="entry__row">
              <span class="entry__row-label">{{ t("module.home.meta.console") }}</span>
              <CoSwitch
                :model-value="current.enable_console"
                :disabled="saving"
                @update:model-value="(value: boolean) => void saveToggle({ enable_console: value })"
              />
            </li>
            <li class="entry__row">
              <span class="entry__row-label">{{ t("module.home.meta.editor_mode") }}</span>
              <CoSwitch
                :model-value="current.enable_editor_mode && current.editor_supported"
                :disabled="saving || !current.editor_supported"
                :title="current.editor_supported ? '' : t('module.home.basic.editor_unsupported')"
                @update:model-value="
                  (value: boolean) => void saveToggle({ enable_editor_mode: value })
                "
              />
            </li>
          </ul>
        </div>
      </section>

      <aside class="entry__actions">
        <section v-for="group in actionGroups" :key="group.key" class="entry__group">
          <span class="entry__group-label">{{ group.label }}</span>
          <button
            v-for="action in group.items"
            :key="action.key"
            type="button"
            :class="[
              'entry__action',
              {
                'entry__action--danger': action.danger,
                'entry__action--disabled': action.disabled,
              },
            ]"
            :disabled="action.disabled"
            :title="action.reason || ''"
            @click="action.run()"
          >
            <component :is="action.icon" :size="15" :stroke-width="1.7" />
            <span>{{ action.label }}</span>
          </button>
        </section>
      </aside>
    </div>

    <div v-else-if="!loading" class="entry__empty">
      <Gamepad2 :size="26" :stroke-width="1.5" />
      <p>{{ t("module.home.empty") }}</p>
      <p class="entry__empty-hint">{{ t("module.home.install_hint") }}</p>
    </div>

    <IconEditDialog
      v-model:open="iconDialogOpen"
      :version-name="currentName"
      :current-logo="current?.logo_data_url ?? null"
      @saved="refresh"
    />
    <InstanceEditDialog
      v-model:open="editDialogOpen"
      :version="current"
      @updated="onUpdated"
      @edit-cover="iconDialogOpen = true"
    />
  </div>
</template>

<style scoped>
.entry {
  height: 100%;
  min-height: 0;
  display: flex;
  overflow-y: auto;
  padding: var(--copper-space-4);
}

/* 主栏吃掉剩余宽度，动作栏固定一档副栏宽度（与全站副栏同宽），
 * 这样中间不会留下一条谁都不属于的空白；卡片纵向拉伸填满内容区。 */
.entry__grid {
  flex: 1;
  /* min-content 而不是 0：内容比视口高时让页面滚动、卡片跟着长高，
   * 否则卡片被压成视口高度、内容溢出到卡片外面。 */
  min-height: min-content;
  display: grid;
  grid-template-columns: minmax(0, 1fr) var(--copper-aside-w-narrow);
  gap: var(--copper-space-5);
  align-items: stretch;
}

/* 信息卡不描边：表面色比页面底色高一档，再加一道很浅的投影就够了。
 * 卡片的作用是把「封面 + 模式 + 开关」收成一组 —— 之前它们直接铺在页面底色上，
 * 三块内容各自与边距对齐，视线没有落点，所以看起来"散"。 */
.entry__main {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-5);
  padding: var(--copper-space-5);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow-subtle);
}

.entry__hero {
  display: flex;
  align-items: center;
  gap: var(--copper-space-4);
}

.entry__cover {
  width: 76px;
  height: 76px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  cursor: pointer;
  overflow: hidden;
  transition: transform var(--copper-duration-fast) var(--copper-easing);
}

.entry__cover:hover {
  transform: scale(1.02);
}

.entry__cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.entry__ident {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.entry__name {
  font-size: 24px;
  font-weight: 600;
  line-height: 1.2;
  letter-spacing: -0.01em;
  overflow-wrap: anywhere;
}

.entry__meta {
  display: flex;
  flex-wrap: wrap;
  gap: var(--copper-space-3);
  font-size: var(--copper-font-size-md);
  color: var(--copper-text-secondary);
}

/* 每组 = 一个标签 + 一组控件。标签是排版锚点：没有它，分段条与开关行在卡片里
 * 只是上下两坨，读不出「这是一组设置」。 */
.entry__section {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  min-width: 0;
}

.entry__section-label {
  font-size: var(--copper-font-size-sm);
  font-weight: 500;
  letter-spacing: 0.02em;
  color: var(--copper-text-secondary);
}

.entry__mode {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  min-width: 0;
}

/* 分段条铺满卡片内容宽度（planes 的 w-full），与下方开关行的分隔线同宽对齐 ——
 * 宽度不一的两块元素并排时，视线会读成"没对齐"，这是页面显散的来源之一。 */
.entry__mode :deep(.co-segmented) {
  display: flex;
  width: 100%;
}

.entry__mode :deep(.co-segmented__item) {
  flex: 1;
}

.entry__hint {
  margin: 0;
  max-width: 60ch;
  font-size: var(--copper-font-size-sm);
  line-height: 1.6;
  color: var(--copper-text-secondary);
}

/* 模式缺条件时的提醒：不是错误，是"切过去也不会生效"。 */
.entry__warning {
  padding: var(--copper-space-2) var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  border: 1px solid color-mix(in srgb, var(--copper-warning) 45%, var(--copper-border));
  background: color-mix(in srgb, var(--copper-warning) 10%, transparent);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  line-height: 1.5;
}

/* 行与行之间不画分隔线：行本身有 52px 的高度和悬停底色，
 * 线是第二重边界，去掉后卡片内部只剩一层"表面 + 间距"的语言。 */
.entry__rows {
  list-style: none;
  display: flex;
  flex-direction: column;
  margin: 0 calc(-1 * var(--copper-space-2));
  padding: 0;
}

.entry__row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  min-height: 52px;
  padding: 0 var(--copper-space-2);
  border-radius: var(--copper-radius-md);
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.entry__row:hover {
  background-image: linear-gradient(var(--copper-hover), var(--copper-hover));
}

.entry__row-label {
  /* 比正文（13px）大一档：52px 的行高里 13px 显得发虚，这一页又是低频操作页，
   * 不需要为了塞信息而压字号。 */
  font-size: 14px;
}

.entry__actions {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-5);
  width: 100%;
  justify-self: stretch;
}

.entry__group {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.entry__group-label {
  padding: 0 var(--copper-space-2) var(--copper-space-1);
  font-size: var(--copper-font-size-sm);
  font-weight: 500;
  color: var(--copper-text-secondary);
}

/* 动作按钮：不描边 —— 靠「表面色 + 细阴影」浮在页面底色上，悬停叠一层主题中性
 * 薄雾（--copper-hover），按下交给全局按压反馈。左右留白 16px，图标与文字之间
 * 留一个间距单位，文字不会贴到边上。 */
.entry__action {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: 36px;
  padding: 0 var(--copper-space-4);
  border: none;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  box-shadow: var(--copper-shadow-raised);
  font-family: inherit;
  font-size: 14px;
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.entry__action svg {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
  transition: color var(--copper-duration-fast) var(--copper-easing);
}

.entry__action:hover:not(:disabled) {
  background-image: linear-gradient(var(--copper-hover), var(--copper-hover));
}

.entry__action:hover:not(:disabled) svg {
  color: var(--copper-text);
}

.entry__action:active:not(:disabled) {
  transform: translateY(1px);
}

.entry__action--danger {
  color: var(--copper-danger);
}

.entry__action--danger svg {
  color: var(--copper-danger);
}

.entry__action--danger:hover:not(:disabled) {
  background-image: none;
  background-color: color-mix(in srgb, var(--copper-danger) 10%, var(--copper-surface));
}

.entry__action--disabled {
  opacity: 0.45;
  box-shadow: none;
  cursor: not-allowed;
}

.entry__empty {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-1);
  color: var(--copper-text-secondary);
  text-align: center;
}

.entry__empty-hint {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
}

/* 窄窗口：动作列落到底部，按钮横排，避免把实例信息挤成一条。 */
@media (max-width: 720px) {
  .entry__grid {
    grid-template-columns: minmax(0, 1fr);
  }

  .entry__actions {
    flex-direction: row;
    flex-wrap: wrap;
    gap: var(--copper-space-3);
    justify-self: stretch;
  }

  .entry__group {
    flex: 1 1 100%;
  }

  .entry__action {
    flex: 1 1 140px;
  }
}
</style>
