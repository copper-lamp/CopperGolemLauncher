<script setup lang="ts">
// 版本设置 · 模组管理分区。
//
// 只管理**模组文件**：目录规范对齐 LeviLauncher / LiteLoader
// （`<版本目录>/mods/<模组文件夹>/manifest.json`）。**不涉及加载器本体**，
// 模组能否真正生效取决于版本内是否已装加载器，本页不做注入 / 预加载。
//
// 列表拉取由后端 `mods.changed` 事件驱动：导入 / 启停 / 删除 / 编辑清单成功后
// 后端都会广播该事件，这里订阅后按版本名过滤刷新，避免多处手写刷新导致重复请求。
// 搜索词由面板工具栏承载（本分区只读不写回）。

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { FileArchive, FolderOpen, Package, Pencil, Puzzle, Trash2, ToggleLeft, ToggleRight } from "@lucide/vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { isKernelApiError } from "../../api/core";
import CoButton from "../../components/ui/CoButton.vue";
import ModEditDialog from "./ModEditDialog.vue";
import {
  homeModsImportDll,
  homeModsImportZip,
  homeModsList,
  homeModsOpenFolder,
  homeModsRemove,
  homeModsSetEnabled,
  type ModView,
} from "../../api/home";
import { onModsChanged } from "../../events";

const props = defineProps<{
  versionName: string;
  /** 面板工具栏的搜索词（由 `VersionSettings` 传入）。 */
  search: string;
}>();

const { t } = useI18n();

const mods = ref<ModView[]>([]);
const skipped = ref(0);
const loading = ref(false);
const errorText = ref("");
const selectedFolder = ref<string | null>(null);
const importing = ref(false);
const editOpen = ref(false);

const selected = computed(
  () => mods.value.find((m) => m.folder === selectedFolder.value) ?? null,
);

const filtered = computed(() => {
  const q = props.search.trim().toLowerCase();
  if (!q) return mods.value;
  return mods.value.filter(
    (m) =>
      m.name.toLowerCase().includes(q) ||
      m.folder.toLowerCase().includes(q) ||
      m.mod_type.toLowerCase().includes(q) ||
      m.author.toLowerCase().includes(q),
  );
});

const emptyText = computed(() =>
  props.search.trim() ? t("module.home.mods.empty_search") : t("module.home.mods.empty"),
);

watch(
  () => props.versionName,
  () => void refresh(),
);

let dispose: (() => void) | null = null;

onMounted(async () => {
  await refresh();
  dispose = await onModsChanged((name) => {
    if (name === props.versionName) void refresh();
  });
});

onUnmounted(() => {
  dispose?.();
  dispose = null;
});

async function refresh() {
  loading.value = true;
  errorText.value = "";
  try {
    const result = await homeModsList(props.versionName);
    mods.value = result.mods;
    skipped.value = result.skipped;
    if (selectedFolder.value && !result.mods.some((m) => m.folder === selectedFolder.value)) {
      selectedFolder.value = null;
    }
  } catch (e) {
    errorText.value = String(e);
    mods.value = [];
    skipped.value = 0;
  } finally {
    loading.value = false;
  }
}

/** 选文件 → 导入；重名时后端返回 `conflict`，确认后再以 `overwrite` 重试。 */
async function runImport(attempt: (overwrite: boolean) => Promise<unknown>) {
  importing.value = true;
  try {
    await attempt(false);
  } catch (e) {
    if (isKernelApiError(e) && e.kind === "conflict") {
      if (!window.confirm(t("module.home.mods.overwrite_confirm"))) return;
      try {
        await attempt(true);
      } catch (retry) {
        showToast(String(retry), "error");
        return;
      }
    } else {
      showToast(String(e), "error");
      return;
    }
  } finally {
    importing.value = false;
  }
  showToast(t("module.home.mods.imported"), "success");
}

async function importZip() {
  const picked = await openDialog({
    multiple: false,
    filters: [{ name: t("module.home.mods.zip_filter"), extensions: ["zip"] }],
  });
  if (typeof picked !== "string" || !picked) return;
  await runImport((overwrite) => homeModsImportZip(props.versionName, picked, overwrite));
}

async function importDll() {
  const picked = await openDialog({
    multiple: false,
    filters: [{ name: t("module.home.mods.dll_filter"), extensions: ["dll"] }],
  });
  if (typeof picked !== "string" || !picked) return;
  // 名称 / 类型 / 版本留空 → 后端按默认值生成清单（名称取文件名词干、类型 preload-native），
  // 需要自定义时可在导入后「编辑清单」。
  await runImport((overwrite) =>
    homeModsImportDll(props.versionName, picked, "", "", "", overwrite),
  );
}

async function openModsFolder() {
  try {
    await homeModsOpenFolder(props.versionName);
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function toggleEnabled() {
  const mod = selected.value;
  if (!mod) return;
  try {
    await homeModsSetEnabled(props.versionName, mod.folder, !mod.enabled);
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function removeSelected() {
  const mod = selected.value;
  if (!mod) return;
  if (!window.confirm(t("module.home.mods.remove_confirm", { name: mod.name }))) return;
  try {
    await homeModsRemove(props.versionName, mod.folder);
    showToast(t("module.home.mods.removed"), "success");
  } catch (e) {
    showToast(String(e), "error");
  }
}
</script>

<template>
  <div class="mod-tab">
    <div class="mod-tab__bar">
      <div class="mod-tab__actions">
        <CoButton variant="secondary" size="sm" :disabled="importing" @click="importZip">
          <FileArchive :size="14" />
          <span>{{ t("module.home.mods.import_zip") }}</span>
        </CoButton>
        <CoButton variant="secondary" size="sm" :disabled="importing" @click="importDll">
          <Puzzle :size="14" />
          <span>{{ t("module.home.mods.import_dll") }}</span>
        </CoButton>
        <CoButton variant="ghost" size="sm" @click="openModsFolder">
          <FolderOpen :size="14" />
          <span>{{ t("module.home.mods.open_folder") }}</span>
        </CoButton>
      </div>
      <span v-if="skipped > 0" class="mod-tab__skipped">
        {{ t("module.home.mods.skipped", { count: skipped }) }}
      </span>
    </div>
    <p class="mod-tab__hint">{{ t("module.home.mods.loader_hint") }}</p>

    <div v-if="loading" class="mod-tab__state">{{ t("common.loading") }}</div>
    <div v-else-if="errorText" class="mod-tab__state">{{ errorText }}</div>
    <div v-else-if="filtered.length === 0" class="mod-tab__state">
      <Package :size="22" :stroke-width="1.5" />
      <span>{{ emptyText }}</span>
    </div>

    <ul v-else class="mod-tab__list">
      <li
        v-for="mod in filtered"
        :key="mod.folder"
        :class="['mod-tab__item', { 'mod-tab__item--selected': selectedFolder === mod.folder }]"
        @click="selectedFolder = selectedFolder === mod.folder ? null : mod.folder"
      >
        <div class="mod-tab__main">
          <span class="mod-tab__name">{{ mod.name }}</span>
          <span class="mod-tab__meta">
            {{ mod.version }} · {{ mod.mod_type }} ·
            {{ mod.author || t("module.home.mods.author_none") }}
          </span>
        </div>
        <span
          :class="[
            'mod-tab__tag',
            mod.enabled ? 'mod-tab__tag--on' : 'mod-tab__tag--off',
          ]"
        >
          {{ mod.enabled ? t("module.home.mods.enabled") : t("module.home.mods.disabled") }}
        </span>
      </li>
    </ul>

    <footer v-if="selected" class="mod-tab__bar-actions">
      <span class="mod-tab__selected-name">{{ selected.name }}</span>
      <div class="mod-tab__selected-actions">
        <CoButton variant="secondary" size="sm" @click="toggleEnabled">
          <ToggleRight v-if="!selected.enabled" :size="14" />
          <ToggleLeft v-else :size="14" />
          <span>
            {{ selected.enabled ? t("module.home.mods.disable") : t("module.home.mods.enable") }}
          </span>
        </CoButton>
        <CoButton variant="secondary" size="sm" @click="editOpen = true">
          <Pencil :size="14" />
          <span>{{ t("module.home.mods.edit") }}</span>
        </CoButton>
        <CoButton variant="danger" size="sm" @click="removeSelected">
          <Trash2 :size="14" />
          <span>{{ t("module.home.mods.remove") }}</span>
        </CoButton>
      </div>
    </footer>

    <ModEditDialog
      v-model:open="editOpen"
      :version-name="versionName"
      :mod="selected"
    />
  </div>
</template>

<style scoped>
.mod-tab {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.mod-tab__bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) var(--copper-space-4) 0;
}

.mod-tab__actions {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  flex-wrap: wrap;
}

.mod-tab__skipped {
  flex-shrink: 0;
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.mod-tab__hint {
  padding: var(--copper-space-2) var(--copper-space-4) 0;
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.mod-tab__list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  list-style: none;
  padding: var(--copper-space-3) var(--copper-space-2) var(--copper-space-2);
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.mod-tab__item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-2) var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.mod-tab__item:hover {
  background: var(--copper-hover);
}

.mod-tab__item--selected {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
}

.mod-tab__main {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.mod-tab__name {
  font-size: var(--copper-font-size-md);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mod-tab__meta {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mod-tab__tag {
  flex-shrink: 0;
  font-size: var(--copper-font-size-xs);
  padding: 2px 8px;
  border-radius: var(--copper-radius-full);
}

.mod-tab__tag--on {
  color: color-mix(in srgb, var(--copper-accent) 80%, var(--copper-text));
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
}

.mod-tab__tag--off {
  color: var(--copper-text-disabled);
  background: var(--copper-surface-2);
}

.mod-tab__state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-2);
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
  padding: var(--copper-space-4);
  text-align: center;
}

.mod-tab__bar-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) var(--copper-space-4);
  border-top: 1px solid var(--copper-border);
  background: var(--copper-surface-2);
  animation: mod-bar-in var(--copper-duration) var(--copper-easing);
}

.mod-tab__selected-name {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mod-tab__selected-actions {
  display: flex;
  gap: var(--copper-space-2);
  flex-shrink: 0;
}

@keyframes mod-bar-in {
  from {
    opacity: 0;
    transform: translateY(4px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}
</style>