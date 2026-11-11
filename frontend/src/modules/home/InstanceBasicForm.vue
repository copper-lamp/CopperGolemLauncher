<script setup lang="ts">
// 实例基本信息表单：封面、版本名、目录快捷方式、启动文件与注入。
//
// 从旧「版本设置 · 基本设置分区」收敛而来：实例模式分段与三个开关已上移到实例入口页，
// 「删除实例」也归入口页的危险动作，这里只留「修改实例」该改的内容。
//
// 写入仍在本组件内完成（便于失败时回滚输入框草稿），成功后经 `updated` 把新视图
// 交回弹窗 / 入口页同步清单与路由。封面走 `edit-cover` 事件，由外层打开封面弹窗 ——
// 裁剪与上传不属于表单逻辑。

import { computed, onMounted, ref, watch } from "vue";
import { FolderOpen, Gamepad2, Puzzle, Upload, RotateCcw, Eraser } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoTextField from "../../components/ui/CoTextField.vue";
import {
  homeLaunchBackupDelete,
  homeLaunchFileRestore,
  homeLaunchFileState,
  homePreloadSummary,
  homeVersionOpenDir,
  homeVersionRename,
  type LaunchFileState,
  type PreloadSummary,
  type VersionDirKind,
  type VersionView,
} from "../../api/home";

const props = defineProps<{
  version: VersionView;
}>();

const emit = defineEmits<{
  /** 重命名成功后回传新视图。 */
  updated: [view: VersionView];
  /** 请求打开封面编辑弹窗。 */
  "edit-cover": [];
}>();

const { t } = useI18n();

const nameDraft = ref(props.version.name);

watch(
  () => props.version.name,
  (name) => {
    nameDraft.value = name;
  },
);

const typeLabel = computed(() => {
  const key = `module.home.type.${props.version.version_type}`;
  const label = t(key);
  return label === key ? props.version.version_type : label;
});

// ---------------------------------------------------------------- 启动文件维护

const launchFile = ref<LaunchFileState | null>(null);
const preload = ref<PreloadSummary | null>(null);
const busy = ref(false);

async function refreshLaunchState() {
  const name = props.version.name;
  if (props.version.compatibility) { launchFile.value = null; preload.value = null; return; }
  try {
    launchFile.value = await homeLaunchFileState(name);
  } catch {
    // 非 Windows / 未注入过：状态拿不到就隐藏该区块，不给用户假信息。
    launchFile.value = null;
  }
  try {
    preload.value = await homePreloadSummary(name);
  } catch {
    preload.value = null;
  }
}

onMounted(() => void refreshLaunchState());
watch(() => props.version.name, () => void refreshLaunchState());

/** 备份文件大小的人类可读文本。 */
const backupLabel = computed(() => {
  const bytes = launchFile.value?.backup_bytes ?? 0;
  if (bytes <= 0) return "";
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
});

/** 预加载说明：外部预加载器接管 / 列出条目 / 无原生模块。 */
const preloadLabel = computed(() => {
  const info = preload.value;
  if (!info) return "";
  if (info.external_preloader) return t("module.home.basic.preload_external");
  if (info.entry_count > 0) {
    return t("module.home.basic.preload_entries", { count: info.entry_count });
  }
  return t("module.home.basic.preload_none");
});

/** 启动文件状态说明：注入是否生效 + 备份情况。 */
const launchFileHint = computed(() => {
  const state = launchFile.value;
  if (!state) return "";
  const injected = state.injected
    ? t("module.home.basic.injected_yes")
    : t("module.home.basic.injected_no");
  return state.backup_exists
    ? `${injected} · ${t("module.home.basic.backup_exists")}`
    : injected;
});

async function restoreLaunchFile() {
  if (props.version.compatibility) return;
  const name = props.version.name;
  busy.value = true;
  try {
    await homeLaunchFileRestore(name);
    showToast(t("module.home.toast.launch_file_restored"), "success");
  } catch (e) {
    showToast(t("module.home.toast.launch_file_restore_failed", { message: String(e) }), "error");
  } finally {
    busy.value = false;
    await refreshLaunchState();
  }
}

async function deleteBackup() {
  if (props.version.compatibility) return;
  const name = props.version.name;
  busy.value = true;
  try {
    await homeLaunchBackupDelete(name);
    showToast(t("module.home.toast.backup_deleted"), "success");
  } catch (e) {
    showToast(t("module.home.toast.backup_delete_failed", { message: String(e) }), "error");
  } finally {
    busy.value = false;
    await refreshLaunchState();
  }
}

/** 提交重命名（值未变时直接还原草稿）。 */
async function saveName() {
  const target = props.version;
  const next = nameDraft.value.trim();
  if (!next || next === target.name) {
    nameDraft.value = target.name;
    return;
  }
  try {
    const updated = await homeVersionRename(target.name, next);
    showToast(t("module.home.toast.renamed"), "success");
    emit("updated", updated);
  } catch (e) {
    showToast(String(e), "error");
    nameDraft.value = target.name;
  }
}

/** 目录快捷方式：图标 + i18n key + 后端目标种类。 */
const dirShortcuts: Array<{
  kind: VersionDirKind;
  labelKey: string;
  icon: typeof FolderOpen;
}> = [
  { kind: "version", labelKey: "module.home.dir.version", icon: FolderOpen },
  { kind: "mods", labelKey: "module.home.dir.mods", icon: Puzzle },
  { kind: "worlds", labelKey: "module.home.dir.worlds", icon: Upload },
];

async function openDir(kind: VersionDirKind) {
  try {
    await homeVersionOpenDir(props.version.name, kind);
  } catch (e) {
    showToast(String(e), "error");
  }
}
</script>

<template>
  <div class="basic">
    <div class="basic__head">
      <button
        class="basic__cover"
        :title="t('module.home.basic.logo_hint')"
        @click="emit('edit-cover')"
      >
        <img v-if="version.logo_data_url" :src="version.logo_data_url" alt="" />
        <Gamepad2 v-else :size="34" :stroke-width="1.3" />
      </button>

      <div class="basic__ident">
        <span class="basic__label">{{ t("module.home.basic.name") }}</span>
        <CoTextField
          v-model="nameDraft"
          :placeholder="t('module.home.rename_placeholder')"
          @enter="saveName"
          @blur="saveName"
        />
        <p class="basic__meta">
          {{ typeLabel }} · {{ version.game_version }}
          <span v-if="version.registered" class="basic__registered">
            · {{ t("module.home.meta.registered") }}
          </span>
        </p>
      </div>
    </div>

    <ul class="basic__rows">
      <li class="basic__row">
        <div class="basic__row-text">
          <span class="basic__row-title">{{ t("module.home.basic.folders") }}</span>
          <span class="basic__row-hint">{{ t("module.home.basic.folders_hint") }}</span>
        </div>
        <div class="basic__shortcuts">
          <button
            v-for="item in dirShortcuts.filter(item => !version.compatibility || item.kind !== 'mods')"
            :key="item.kind"
            type="button"
            class="basic__shortcut"
            @click="openDir(item.kind)"
          >
            <component :is="item.icon" :size="14" />
            <span>{{ t(item.labelKey) }}</span>
          </button>
        </div>
      </li>
      <li v-if="launchFile && !version.compatibility" class="basic__row">
        <div class="basic__row-text">
          <span class="basic__row-title">{{ t("module.home.basic.launch_file") }}</span>
          <span class="basic__row-hint">{{ launchFileHint }}</span>
          <span v-if="preloadLabel" class="basic__row-hint">{{ preloadLabel }}</span>
        </div>
        <div class="basic__shortcuts">
          <button
            type="button"
            class="basic__shortcut"
            :disabled="!launchFile.backup_exists || busy"
            @click="restoreLaunchFile"
          >
            <RotateCcw :size="14" />
            <span>{{ t("module.home.basic.restore_launch_file") }}</span>
          </button>
          <button
            v-if="launchFile.backup_exists"
            type="button"
            class="basic__shortcut"
            :disabled="busy"
            @click="deleteBackup"
          >
            <Eraser :size="14" />
            <span>
              {{ t("module.home.basic.delete_backup") }}{{ backupLabel ? ` (${backupLabel})` : "" }}
            </span>
          </button>
        </div>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.basic {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-4);
  padding: var(--copper-space-4);
  overflow-y: auto;
}

.basic__head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-4);
}

.basic__cover {
  width: 84px;
  height: 84px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  cursor: pointer;
  overflow: hidden;
  transition:
    border-color var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.basic__cover:hover {
  border-color: var(--copper-accent);
  transform: scale(1.02);
}

.basic__cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.basic__ident {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

/* 字段标签为辅助信息，弱化处理以突出用户输入内容。 */
.basic__label {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.basic__meta {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-secondary);
}

.basic__registered {
  color: color-mix(in srgb, var(--copper-accent) 85%, var(--copper-text));
}

.basic__rows {
  list-style: none;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--copper-border);
}

.basic__row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) 0;
  border-bottom: 1px solid var(--copper-border);
}

.basic__row-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

/* 行标题为主信息：正文色 + 中等字重，与右侧控件对齐基线。 */
.basic__row-title {
  font-size: var(--copper-font-size-md);
  font-weight: 500;
  color: var(--copper-text);
}

/* 行副标题为辅助信息：更小字号 + 弱化色，避免与标题争夺注意力。 */
.basic__row-hint {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.basic__shortcuts {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: var(--copper-space-2);
}

/* 次按钮：低于主操作视觉权重，仅在 hover 时提升到正文色。 */
.basic__shortcut {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.basic__shortcut:hover {
  border-color: var(--copper-accent);
  background: var(--copper-surface-3);
  color: var(--copper-text);
}
</style>
