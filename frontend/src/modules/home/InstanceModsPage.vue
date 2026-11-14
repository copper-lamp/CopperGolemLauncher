<script setup lang="ts">
// 实例 · 模组页（路由 `/version-settings/mods`）：左边是整列模组，右边是操作面板。
//
// 版面 6:4（宽屏）/ 单列（窄屏），与资源页同一套语言：左栏不套卡片、不设页头，
// 筛选按钮平铺在列表顶部；右栏顶部是独立搜索框，下面是操作面板。
//
// 选中是多选：点一行就切换该行的选中状态（不需要按 Ctrl —— 启动器里没有
// 文件管理器的拖框习惯，改成"点一下选一个"更符合直觉）。批量操作都走后端
// 的 `*_many` 命令，一次广播一次刷新，避免 N 个模组触发 N 次列表请求。
//
// 列表由后端 `mods.changed` 事件驱动刷新：导入 / 启停 / 删除 / 编辑清单
// 成功后后端都会广播，这里订阅后按版本名过滤，避免多处手写刷新导致状态打架。

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { Package } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { isKernelApiError } from "../../api/core";
import ModEditDialog from "./ModEditDialog.vue";
import ModRow from "./ModRow.vue";
import ModsPanel from "./ModsPanel.vue";
import InstanceSearchBox from "./InstanceSearchBox.vue";
import {
  homeModsApplyUpdates,
  homeModsImportDll,
  homeModsImportZip,
  homeModsList,
  homeModsOpenFolder,
  homeModsRemove,
  homeModsRemoveMany,
  homeModsSetEnabled,
  homeModsSetEnabledMany,
  homeModsUpdates,
  type ModView,
} from "../../api/home";
import { onModsChanged } from "../../events";
import { useVersionInstance } from "./useVersionInstance";
import { useCompatibilityText } from '../../services/compatibility-messages';
import { open as openDialog } from "@tauri-apps/plugin-dialog";

type ModFilter = "all" | "enabled" | "disabled" | "updatable";

const { t } = useI18n();
const router = useRouter();
const { currentName, current } = useVersionInstance();
const compatText = useCompatibilityText();

const mods = ref<ModView[]>([]);
const skipped = ref(0);
const loading = ref(false);
const errorText = ref("");
const search = ref("");
const filter = ref<ModFilter>("all");
const selectedFolders = ref<string[]>([]);
const updates = ref<Record<string, string>>({});
const importing = ref(false);
const updating = ref(false);
const editOpen = ref(false);

/** 编辑清单的目标（单选语义：悬停行上的「详情/编辑」都指向当前单选的那一个）。 */
const editTarget = computed(
  () => mods.value.find((mod) => mod.folder === selectedFolders.value[0]) ?? null,
);

const updateCount = computed(() => Object.keys(updates.value).length);

const counts = computed(() => ({
  all: mods.value.length,
  enabled: mods.value.filter((mod) => mod.enabled).length,
  disabled: mods.value.filter((mod) => !mod.enabled).length,
  updatable: updateCount.value,
}));

const filters = computed(() => [
  { value: "all" as ModFilter, label: t("module.home.mods.filters.all"), count: counts.value.all },
  {
    value: "enabled" as ModFilter,
    label: t("module.home.mods.filters.enabled"),
    count: counts.value.enabled,
  },
  {
    value: "disabled" as ModFilter,
    label: t("module.home.mods.filters.disabled"),
    count: counts.value.disabled,
  },
  {
    value: "updatable" as ModFilter,
    label: t("module.home.mods.filters.updatable"),
    count: counts.value.updatable,
  },
]);

const filtered = computed(() => {
  const query = search.value.trim().toLowerCase();
  return mods.value.filter((mod) => {
    if (filter.value === "enabled" && !mod.enabled) return false;
    if (filter.value === "disabled" && mod.enabled) return false;
    if (filter.value === "updatable" && !updates.value[mod.folder]) return false;
    if (!query) return true;
    return (
      mod.name.toLowerCase().includes(query) ||
      mod.folder.toLowerCase().includes(query) ||
      mod.mod_type.toLowerCase().includes(query) ||
      mod.author.toLowerCase().includes(query) ||
      mod.description.toLowerCase().includes(query)
    );
  });
});

const selectedMods = computed(() =>
  mods.value.filter((mod) => selectedFolders.value.includes(mod.folder)),
);

const selectedUpdatable = computed(
  () => selectedMods.value.filter((mod) => updates.value[mod.folder]).length,
);

const emptyText = computed(() => {
  if (search.value.trim()) return t("module.home.mods.empty_search");
  if (filter.value !== "all") return t("module.home.mods.empty_filter");
  return t("module.home.mods.empty");
});

async function refresh() {
  if (current.value?.compatibility && !current.value.compatibility.capabilities.loader) { mods.value = []; selectedFolders.value = []; loading.value = false; return; }
  // 实例名还没解析出来（刚进页面 / 刚切实例）：空名字会被后端拒绝成「版本名不能为空」，
  // 那个错误还会晚于随后成功的请求落到 UI，把已经读到的列表盖成一条报错。
  const name = currentName.value;
  if (!name) {
    mods.value = [];
    skipped.value = 0;
    selectedFolders.value = [];
    errorText.value = "";
    loading.value = false;
    return;
  }
  loading.value = true;
  errorText.value = "";
  try {
    const result = await homeModsList(name);
    // 请求期间实例已切换：丢弃过期结果，别让旧实例的列表盖掉新实例。
    if (name !== currentName.value) return;
    mods.value = result.mods;
    skipped.value = result.skipped;
    // 选中项可能已被删除或改名。
    selectedFolders.value = selectedFolders.value.filter((folder) =>
      result.mods.some((mod) => mod.folder === folder),
    );
  } catch (e) {
    if (name !== currentName.value) return;
    errorText.value = String(e);
    mods.value = [];
    skipped.value = 0;
    selectedFolders.value = [];
  } finally {
    if (name === currentName.value) loading.value = false;
  }
  if (name !== currentName.value) return;
  await refreshUpdates(name);
}

/** 更新检查是网络请求：列表先渲染，徽标后补，不阻塞首屏。 */
async function refreshUpdates(name: string) {
  try {
    const list = await homeModsUpdates(name);
    if (name !== currentName.value) return;
    const map: Record<string, string> = {};
    for (const update of list) map[update.folder] = update.latest_version;
    updates.value = map;
  } catch {
    updates.value = {};
  }
}

let dispose: (() => void) | null = null;

onMounted(() => {
  void refresh();
  dispose = null;
  void onModsChanged((name) => {
    if (name === currentName.value) void refresh();
  }).then((off) => {
    dispose = off;
  });
});

onUnmounted(() => {
  dispose?.();
  dispose = null;
});

watch(currentName, () => {
  selectedFolders.value = [];
  void refresh();
});

function isSelected(folder: string): boolean {
  return selectedFolders.value.includes(folder);
}

function toggle(folder: string) {
  selectedFolders.value = isSelected(folder)
    ? selectedFolders.value.filter((item) => item !== folder)
    : [...selectedFolders.value, folder];
}

function clearSelection() {
  selectedFolders.value = [];
}

function selectOnly(folder: string) {
  selectedFolders.value = [folder];
}

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

async function importZipPath(path: string) {
  await runImport((overwrite) => homeModsImportZip(currentName.value, path, overwrite));
}

async function importZipDialog() {
  const picked = await openDialog({
    multiple: false,
    filters: [{ name: t("module.home.mods.zip_filter"), extensions: ["zip"] }],
  });
  if (typeof picked !== "string" || !picked) return;
  await importZipPath(picked);
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
    homeModsImportDll(currentName.value, picked, "", "", "", overwrite),
  );
}

async function openFolder() {
  try {
    await homeModsOpenFolder(currentName.value);
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function setEnabled(mod: ModView, enabled: boolean) {
  try {
    await homeModsSetEnabled(currentName.value, mod.folder, enabled);
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function remove(mod: ModView) {
  if (!window.confirm(t("module.home.mods.remove_confirm", { name: mod.name }))) return;
  try {
    await homeModsRemove(currentName.value, mod.folder);
    showToast(t("module.home.mods.removed"), "success");
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function applyUpdates(folders: string[]) {
  if (folders.length === 0) return;
  updating.value = true;
  try {
    const queued = await homeModsApplyUpdates(currentName.value, folders);
    showToast(t("module.home.mods.update_queued", { count: queued }), "success");
    selectedFolders.value = [];
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    updating.value = false;
  }
}

function updateAll() {
  void applyUpdates(Object.keys(updates.value));
}

function updateSelected() {
  void applyUpdates(
    selectedMods.value.filter((mod) => updates.value[mod.folder]).map((mod) => mod.folder),
  );
}

async function setEnabledMany(enabled: boolean) {
  const folders = selectedFolders.value;
  if (folders.length === 0) return;
  try {
    const changed = await homeModsSetEnabledMany(currentName.value, folders, enabled);
    showToast(
      t(enabled ? "module.home.mods.enabled_many" : "module.home.mods.disabled_many", {
        count: changed,
      }),
      "success",
    );
    selectedFolders.value = [];
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function removeMany() {
  const folders = selectedFolders.value;
  if (folders.length === 0) return;
  if (!window.confirm(t("module.home.mods.remove_many_confirm", { count: folders.length }))) return;
  try {
    const removed = await homeModsRemoveMany(currentName.value, folders);
    showToast(t("module.home.mods.removed_many", { count: removed }), "success");
    selectedFolders.value = [];
  } catch (e) {
    showToast(String(e), "error");
  }
}

/** 详情跳转到内容下载的详情页；手动导入的模组没有来源，给出说明而不是空跳。 */
function openDetail(mod: ModView) {
  if (!mod.source_id) {
    showToast(t("module.home.mods.detail_unavailable"), "info");
    return;
  }
  void router.push(`/content/${mod.source_id}`);
}

function editMod(mod: ModView) {
  selectOnly(mod.folder);
  editOpen.value = true;
}
</script>

<template>
  <p v-if="current?.compatibility && !current.compatibility.capabilities.loader" role="status">{{ compatText('unsupported') }}</p>
  <div v-else class="mods">
    <section class="mods__main">
      <div class="mods__filters" role="tablist">
        <button
          v-for="item in filters"
          :key="item.value"
          type="button"
          role="tab"
          :aria-selected="filter === item.value"
          :class="['chip', { 'chip--active': filter === item.value }]"
          @click="filter = item.value"
        >
          <span>{{ item.label }}</span>
          <span class="chip__count">{{ item.count }}</span>
        </button>
      </div>

      <div class="mods__body">
        <div v-if="loading" class="mods__state">{{ t("common.loading") }}</div>
        <div v-else-if="errorText" class="mods__state">{{ errorText }}</div>
        <div v-else-if="filtered.length === 0" class="mods__state">
          <Package :size="22" :stroke-width="1.5" />
          <span>{{ emptyText }}</span>
        </div>
        <div v-else class="mods__list" role="listbox" aria-multiselectable="true">
          <ModRow
            v-for="mod in filtered"
            :key="mod.folder"
            :mod="mod"
            :selected="isSelected(mod.folder)"
            :update-version="updates[mod.folder] ?? null"
            @toggle="toggle(mod.folder)"
            @detail="openDetail(mod)"
            @folder="openFolder"
            @enable="setEnabled(mod, !mod.enabled)"
            @remove="remove(mod)"
          />
        </div>
      </div>

      <p v-if="skipped > 0" class="mods__skipped">
        {{ t("module.home.mods.skipped", { count: skipped }) }}
      </p>
    </section>

    <aside class="mods__aside">
      <InstanceSearchBox
        v-model="search"
        :placeholder="t('module.home.mods.search_placeholder')"
      />
      <ModsPanel
        :selected-count="selectedFolders.length"
        :can-edit="selectedFolders.length === 1"
        :selected-updatable="selectedUpdatable"
        :updatable-count="updateCount"
        :importing="importing"
        :updating="updating"
        @open-folder="openFolder"
        @import-zip="importZipPath"
        @import-zip-dialog="importZipDialog"
        @import-dll="importDll"
        @edit="editTarget && editMod(editTarget)"
        @update-all="updateAll"
        @update-selected="updateSelected"
        @download="router.push('/content')"
        @enable="setEnabledMany(true)"
        @disable="setEnabledMany(false)"
        @remove="removeMany"
        @clear="clearSelection"
        @reject-drop="showToast(t('module.home.mods.drop_not_zip'), 'error')"
      />
    </aside>

    <ModEditDialog v-model:open="editOpen" :version-name="currentName" :mod="editTarget" />
  </div>
</template>

<style scoped>
.mods {
  display: grid;
  grid-template-columns: minmax(0, 6fr) minmax(0, 4fr);
  gap: var(--copper-space-4);
  height: 100%;
  min-height: 0;
  padding: var(--copper-space-4);
}

/* 左：筛选按钮 + 整列模组。不套卡片，靠行底色区分层次。 */
.mods__main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  gap: var(--copper-space-3);
}

.mods__filters {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--copper-space-1);
}

/* 平铺的筛选胶囊：选中项用强调色的微色阶，不用蓝底白字（主题是铜色）。 */
.chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-full);
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-md);
  font-weight: 500;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.chip:hover {
  background: var(--copper-surface-2);
  color: var(--copper-text);
}

.chip--active {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
  color: color-mix(in srgb, var(--copper-accent) 85%, var(--copper-text));
  font-weight: 600;
}

.chip__count {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
  font-variant-numeric: tabular-nums;
}

.chip--active .chip__count {
  color: inherit;
  opacity: 0.75;
}

.mods__body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.mods__list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding-bottom: var(--copper-space-4);
}

.mods__state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-2);
  padding: var(--copper-space-6);
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
  text-align: center;
}

.mods__skipped {
  flex-shrink: 0;
  padding: 0 var(--copper-space-2);
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

/* 右：搜索独立在顶部，下面是操作面板。 */
.mods__aside {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  min-width: 0;
  min-height: 0;
  overflow-y: auto;
}

@media (max-width: 900px) {
  .mods {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr) auto;
    overflow-y: auto;
  }

  .mods__aside {
    overflow: visible;
  }
}
</style>