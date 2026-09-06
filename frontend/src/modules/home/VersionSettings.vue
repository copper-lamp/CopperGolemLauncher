<script setup lang="ts">
// 版本设置页：左右布局。
// - 左：版本 Tabs（图标 / 版本名 / 版本号），卡片包裹选中项；
// - 右：版本信息 —— 封面（点击编辑）+ 版本名编辑，下方 版本文件夹 / 渲染龙 /
//       世界编辑器 / 删除版本，底部为内容管理（ContentPanel）。

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import {
  ChevronLeft,
  FolderOpen,
  Gamepad2,
  Trash2,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoButton from "../../components/ui/CoButton.vue";
import CoSwitch from "../../components/ui/CoSwitch.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import ContentPanel from "./ContentPanel.vue";
import IconEditDialog from "./IconEditDialog.vue";
import {
  homeVersionDelete,
  homeVersionGet,
  homeVersionRename,
  homeVersionSaveMeta,
  homeVersionsList,
  type VersionView,
} from "../../api/home";
import { onVersionInstalled, onVersionRemoved } from "../../events";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();

const versions = ref<VersionView[]>([]);
const current = ref<VersionView | null>(null);
const nameDraft = ref("");
const editingName = ref(false);
const iconDialogOpen = ref(false);
const loading = ref(true);

const versionOptions = computed(() => versions.value);

const currentName = computed(() => current.value?.name ?? "");

watch(
  () => route.query.name,
  (name) => {
    const target = name && typeof name === "string" ? name : versions.value[0]?.name;
    if (target) void selectVersion(target);
  },
);

onMounted(async () => {
  await refresh();
  let unlisten: Array<() => void> = [];
  unlisten = [
    await onVersionInstalled(() => void refresh()),
    await onVersionRemoved(() => void refresh()),
  ];
  dispose = unlisten;
});

let dispose: Array<() => void> = [];

onUnmounted(() => {
  dispose.forEach((u) => u());
  dispose = [];
});

async function refresh() {
  try {
    versions.value = await homeVersionsList();
    const queryName = route.query.name;
    const target =
      (typeof queryName === "string" && queryName) ||
      versions.value[0]?.name ||
      "";
    if (target) {
      await selectVersion(target);
    } else {
      current.value = null;
    }
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    loading.value = false;
  }
}

async function selectVersion(name: string) {
  try {
    current.value = await homeVersionGet(name);
    nameDraft.value = current.value.name;
    editingName.value = false;
  } catch (e) {
    showToast(String(e), "error");
  }
}

function pickVersion(name: string) {
  if (name === currentName.value) return;
  void router.replace({ path: "/version-settings", query: { name } });
}

async function saveName() {
  const target = current.value;
  if (!target || editingName.value === false) return;
  const newName = nameDraft.value.trim();
  editingName.value = false;
  if (!newName || newName === target.name) {
    nameDraft.value = target.name;
    return;
  }
  try {
    const updated = await homeVersionRename(target.name, newName);
    showToast(t("home.toast.renamed"), "success");
    await refresh();
    void router.replace({ path: "/version-settings", query: { name: updated.name } });
  } catch (e) {
    showToast(String(e), "error");
    nameDraft.value = target.name;
  }
}

async function saveMeta(update: { enable_render_dragon?: boolean; enable_editor_mode?: boolean }) {
  const target = current.value;
  if (!target) return;
  try {
    const updated = await homeVersionSaveMeta(target.name, update);
    current.value = updated;
    const index = versions.value.findIndex((v) => v.name === target.name);
    if (index >= 0) versions.value[index] = updated;
  } catch (e) {
    showToast(String(e), "error");
  }
}

function openFolder() {
  if (!current.value) return;
  void import("@tauri-apps/plugin-opener").then(({ openPath }) =>
    openPath(current.value!.folder),
  );
}

async function deleteVersion() {
  const target = current.value;
  if (!target) return;
  if (!window.confirm(t("home.delete_confirm", { name: target.name }))) return;
  try {
    await homeVersionDelete(target.name);
    showToast(t("home.toast.deleted"), "success");
    const remaining = versions.value.filter((v) => v.name !== target.name);
    if (remaining.length === 0) {
      void router.push("/");
      return;
    }
    await refresh();
    void router.replace({
      path: "/version-settings",
      query: { name: remaining[0].name },
    });
  } catch (e) {
    showToast(String(e), "error");
  }
}

function goBack() {
  void router.push("/");
}

function typeLabel(type: string): string {
  const key = `home.type.${type}`;
  const label = t(key);
  return label === key ? type : label;
}
</script>

<template>
  <div class="version-settings">
    <!-- 左：版本 Tabs -->
    <aside class="version-settings__tabs">
      <header class="version-settings__tabs-head">
        <button class="version-settings__back" :title="t('home.back')" @click="goBack">
          <ChevronLeft :size="18" />
        </button>
        <span class="version-settings__tabs-title">{{ t("home.settings") }}</span>
      </header>

      <div v-if="loading" class="version-settings__state">
        {{ t("common.loading") }}
      </div>
      <div v-else-if="versions.length === 0" class="version-settings__state">
        <Gamepad2 :size="26" :stroke-width="1.5" />
        <span>{{ t("home.empty") }}</span>
      </div>

      <div v-else class="version-settings__tabs-list">
        <button
          v-for="v in versionOptions"
          :key="v.name"
          :class="[
            'version-settings__tab',
            { 'version-settings__tab--active': v.name === currentName },
          ]"
          @click="pickVersion(v.name)"
        >
          <span class="version-settings__tab-icon">
            <img v-if="v.logo_data_url" :src="v.logo_data_url" alt="" />
            <Gamepad2 v-else :size="18" :stroke-width="1.6" />
          </span>
          <span class="version-settings__tab-text">
            <span class="version-settings__tab-name">{{ v.name }}</span>
            <span class="version-settings__tab-meta">{{ v.game_version }}</span>
          </span>
        </button>
      </div>
    </aside>

    <!-- 右：版本信息 -->
    <section v-if="current" class="version-settings__detail">
      <div class="version-settings__head">
        <button
          class="version-settings__icon"
          :title="t('home.logo.set')"
          @click="iconDialogOpen = true"
        >
          <img v-if="current.logo_data_url" :src="current.logo_data_url" alt="" />
          <Gamepad2 v-else :size="40" :stroke-width="1.3" />
        </button>
        <div class="version-settings__name">
          <CoTextField
            :model-value="nameDraft"
            :placeholder="t('home.rename_placeholder')"
            @update:model-value="(v: string) => { nameDraft = v; editingName = true; }"
            @enter="saveName"
            @blur="saveName"
          />
          <p class="version-settings__meta">
            {{ typeLabel(current.version_type) }} · {{ current.game_version }}
            <span v-if="current.registered" class="version-settings__registered">
              · {{ t("home.meta.registered") }}
            </span>
          </p>
        </div>
      </div>

      <div class="version-settings__controls">
        <CoButton variant="secondary" size="sm" @click="openFolder">
          <FolderOpen :size="14" />
          <span>{{ t("home.open_folder") }}</span>
        </CoButton>
        <label class="version-settings__toggle">
          <span>{{ t("home.meta.render_dragon") }}</span>
          <CoSwitch
            :model-value="current.enable_render_dragon"
            @update:model-value="(v: boolean) => void saveMeta({ enable_render_dragon: v })"
          />
        </label>
        <label class="version-settings__toggle">
          <span>{{ t("home.meta.editor_mode") }}</span>
          <CoSwitch
            :model-value="current.enable_editor_mode"
            @update:model-value="(v: boolean) => void saveMeta({ enable_editor_mode: v })"
          />
        </label>
        <CoButton variant="danger" size="sm" @click="deleteVersion">
          <Trash2 :size="14" />
          <span>{{ t("home.delete") }}</span>
        </CoButton>
      </div>

      <ContentPanel :version-name="currentName" />
    </section>

    <section v-else class="version-settings__detail version-settings__detail--empty">
      <p>{{ t("home.empty") }}</p>
      <p class="version-settings__hint">{{ t("home.install_hint") }}</p>
    </section>

    <IconEditDialog
      v-model:open="iconDialogOpen"
      :version-name="currentName"
      :current-logo="current?.logo_data_url ?? null"
      @saved="refresh"
    />
  </div>
</template>

<style scoped>
.version-settings {
  display: flex;
  gap: var(--copper-space-4);
  height: 100%;
  padding: var(--copper-space-4);
  min-height: 0;
}

.version-settings__tabs {
  width: 232px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  min-height: 0;
  overflow: hidden;
}

.version-settings__tabs-head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
  padding: var(--copper-space-2) var(--copper-space-2) var(--copper-space-3);
  border-bottom: 1px solid var(--copper-border);
}

.version-settings__back {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.version-settings__back:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.version-settings__tabs-title {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
}

.version-settings__tabs-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: var(--copper-space-2);
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.version-settings__tab {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-2);
  border: 1px solid transparent;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text);
  cursor: pointer;
  text-align: left;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing);
}

.version-settings__tab:hover {
  background: var(--copper-hover);
}

.version-settings__tab--active {
  background: color-mix(in srgb, var(--copper-accent) 12%, transparent);
  border-color: color-mix(in srgb, var(--copper-accent) 45%, transparent);
}

.version-settings__tab-icon {
  width: 40px;
  height: 40px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  overflow: hidden;
}

.version-settings__tab-icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.version-settings__tab-text {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.version-settings__tab-name {
  font-size: var(--copper-font-size-md);
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.version-settings__tab-meta {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.version-settings__detail {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-4);
}

.version-settings__detail--empty {
  align-items: center;
  justify-content: center;
  color: var(--copper-text-secondary);
}

.version-settings__head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-4);
}

.version-settings__icon {
  width: 88px;
  height: 88px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  color: var(--copper-text-secondary);
  cursor: pointer;
  overflow: hidden;
  transition:
    border-color var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.version-settings__icon:hover {
  border-color: var(--copper-accent);
  transform: scale(1.02);
}

.version-settings__icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.version-settings__name {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.version-settings__meta {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.version-settings__registered {
  color: color-mix(in srgb, var(--copper-accent) 85%, var(--copper-text));
}

.version-settings__controls {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  flex-wrap: wrap;
}

.version-settings__toggle {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-2);
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
  cursor: pointer;
  user-select: none;
}

.version-settings__state {
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

.version-settings__hint {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
}
</style>
