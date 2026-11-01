<script setup lang="ts">
// 实例 · 客户端页（路由 /version-settings/client）。
//
// 这是开始页模块内部的客户端功能，仅 Windows：导入 / 命名客户端 DLL、在本地
// 客户端列表里点击切换当前实例使用的客户端、删除、启用客户端模式，以及查看
// 启动文件与预加载状态。客户端文件由启动器数据目录保管，实例只记 client_id。
//
// 客户端模式是实例属性（VersionMeta.mode=client + client_id），不是启动器设置：
// 切换实例才会切换客户端组合。

import { computed, onMounted, ref, watch } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  Check,
  FileWarning,
  PlugZap,
  Rocket,
  Trash2,
  Upload,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { usePlatform } from "../../composables/usePlatform";
import CoButton from "../../components/ui/CoButton.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import InstanceFilterList from "./InstanceFilterList.vue";
import InstanceManageShell from "./InstanceManageShell.vue";
import {
  homeClientDelete,
  homeClientImport,
  homeClientsList,
  homeLaunchFileState,
  homePreloadSummary,
  homeVersionSaveMeta,
  type ClientView,
  type LaunchFileState,
  type PreloadSummary,
} from "../../api/home";
import { useVersionInstance } from "./useVersionInstance";

const { t } = useI18n();
const { isWindows } = usePlatform();
const { current, currentName, applyView } = useVersionInstance();

const category = ref("dll");
const busy = ref(false);
const clients = ref<ClientView[]>([]);
const libraryLoading = ref(true);
const launchState = ref<LaunchFileState | null>(null);
const preload = ref<PreloadSummary | null>(null);

/** 已选好文件、等待命名的导入表单。 */
const importOpen = ref(false);
const importPath = ref("");
const importName = ref("");
const importError = ref("");
const importing = ref(false);

const categories = computed(() => [
  { value: "dll", label: t("module.home.client.category.dll"), icon: PlugZap },
  { value: "launch", label: t("module.home.client.category.launch"), icon: Rocket },
]);

const title = computed(() =>
  category.value === "dll"
    ? t("module.home.client.category.dll")
    : t("module.home.client.category.launch"),
);

const clientMode = computed(() => current.value?.mode === "client");

const selectedClient = computed(() =>
  clients.value.find((client) => client.id === current.value?.client_id) ?? null,
);

const selectedLabel = computed(
  () => selectedClient.value?.name ?? t("module.home.client.none"),
);

const backupLabel = computed(() => {
  const state = launchState.value;
  if (!state) return "";
  return state.backup_exists
    ? t("module.home.client.launch_backup_some", { size: formatBytes(state.backup_bytes) })
    : t("module.home.client.launch_backup_none");
});

const preloadLabel = computed(() => {
  const info = preload.value;
  if (!info) return "";
  if (info.external_preloader) return t("module.home.client.launch_preload_external");
  if (info.entry_count > 0) {
    return t("module.home.client.launch_preload_count", { count: info.entry_count });
  }
  return t("module.home.client.launch_preload_none");
});

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 MB";
  return (bytes / 1024 / 1024).toFixed(1) + " MB";
}

function formatDate(millis: number): string {
  if (!millis) return "";
  const date = new Date(millis);
  const pad = (value: number) => String(value).padStart(2, "0");
  return date.getFullYear() + "." + pad(date.getMonth() + 1) + "." + pad(date.getDate());
}

async function loadLibrary() {
  if (!isWindows.value) {
    clients.value = [];
    libraryLoading.value = false;
    return;
  }
  libraryLoading.value = true;
  try {
    clients.value = await homeClientsList();
  } catch (e) {
    clients.value = [];
    showToast(String(e), "error");
  } finally {
    libraryLoading.value = false;
  }
}

async function refreshLaunchState() {
  const name = currentName.value;
  if (!name) {
    launchState.value = null;
    preload.value = null;
    return;
  }
  try {
    launchState.value = await homeLaunchFileState(name);
  } catch {
    launchState.value = null;
  }
  try {
    preload.value = await homePreloadSummary(name);
  } catch {
    preload.value = null;
  }
}

async function saveMeta(update: { mode?: string; client_id?: string }) {
  const version = current.value;
  if (!version) return false;
  if (!isWindows.value) {
    showToast(t("module.home.client.windows_only"), "error");
    return false;
  }
  busy.value = true;
  try {
    applyView(await homeVersionSaveMeta(version.name, update));
    return true;
  } catch (e) {
    showToast(String(e), "error");
    return false;
  } finally {
    busy.value = false;
  }
}

/** 点击列表项：把客户端写进当前实例。 */
async function assignClient(client: ClientView) {
  if (busy.value || !client.available) return;
  if (current.value?.client_id === client.id) return;
  if (await saveMeta({ client_id: client.id })) {
    showToast(t("module.home.client.selected_saved"), "success");
  }
}

async function enableClientMode() {
  const fallback = clients.value.find((client) => client.available)?.id ?? "";
  const clientId = current.value?.client_id || fallback;
  if (await saveMeta({ mode: "client", client_id: clientId })) {
    showToast(t("module.home.client.mode_on"), "success");
  }
}

async function clearClient() {
  if (await saveMeta({ client_id: "" })) {
    showToast(t("module.home.client.cleared"), "success");
  }
}

async function pickFile() {
  if (busy.value || !isWindows.value) return;
  const picked = await openDialog({
    multiple: false,
    filters: [{ name: t("module.home.client.dll_filter"), extensions: ["dll"] }],
  });
  if (typeof picked !== "string" || !picked) return;
  const fileName = picked.split(/[\\/]/).pop() ?? "client.dll";
  importPath.value = picked;
  importName.value = fileName.replace(/\.dll$/i, "");
  importError.value = "";
  importOpen.value = true;
}

async function confirmImport() {
  if (importing.value) return;
  importing.value = true;
  importError.value = "";
  try {
    const view = await homeClientImport(importPath.value, importName.value);
    importOpen.value = false;
    await loadLibrary();
    // 当前实例还没选客户端时自动选上新导入的这个；已选则保持不动。
    if (!current.value?.client_id) {
      await saveMeta({ client_id: view.id });
    }
    showToast(t("module.home.client.imported"), "success");
  } catch (e) {
    importError.value = String(e);
  } finally {
    importing.value = false;
  }
}

function cancelImport() {
  if (importing.value) return;
  importOpen.value = false;
  importError.value = "";
}

async function removeClient(client: ClientView) {
  if (busy.value) return;
  if (!window.confirm(t("module.home.client.delete_confirm", { name: client.name }))) return;
  busy.value = true;
  try {
    await homeClientDelete(client.id);
    // 删掉的正好是本实例选中的客户端：清掉引用，避免下次启动找不到记录。
    if (current.value?.client_id === client.id) {
      const updated = await homeVersionSaveMeta(current.value.name, { client_id: "" });
      applyView(updated);
    }
    await loadLibrary();
    showToast(t("module.home.client.deleted"), "success");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}

watch(currentName, () => {
  void loadLibrary();
  void refreshLaunchState();
});

onMounted(() => {
  void loadLibrary();
  void refreshLaunchState();
});
</script>

<template>
  <InstanceManageShell :title="title">
    <div class="client">
      <template v-if="category === 'dll'">
        <p v-if="!isWindows" class="client__hint">
          {{ t("module.home.client.windows_only") }}
        </p>

        <template v-else>
          <p class="client__hint">{{ t("module.home.client.library_hint") }}</p>

          <div class="client__current">
            <span class="client__current-label">{{ t("module.home.client.current") }}</span>
            <span :class="['client__current-name', { 'client__current-name--empty': !selectedClient }]">
              {{ selectedLabel }}
            </span>
            <span :class="['client__badge', { 'client__badge--on': clientMode }]">
              {{ t("module.home.instance.mode.client") }} · {{ clientMode ? t("module.home.client.mode_on_short") : t("module.home.client.mode_off_short") }}
            </span>
          </div>

          <div v-if="!clientMode" class="client__warn">
            <span>{{ t("module.home.client.not_client_mode") }}</span>
            <CoButton
              variant="secondary"
              size="sm"
              :disabled="busy || !clients.some((item) => item.available)"
              @click="enableClientMode"
            >
              {{ t("module.home.client.enable_mode") }}
            </CoButton>
          </div>
          <div v-else-if="!selectedClient" class="client__warn">
            <span>{{ t("module.home.client.mode_no_client") }}</span>
          </div>

          <div class="client__list-head">
            <span class="client__list-title">{{ t("module.home.client.library") }}</span>
            <CoButton variant="primary" size="sm" :disabled="busy || libraryLoading" @click="pickFile">
              <Upload :size="14" />
              <span>{{ t("module.home.client.import") }}</span>
            </CoButton>
          </div>

          <p v-if="libraryLoading" class="client__state">{{ t("common.loading") }}</p>
          <p v-else-if="clients.length === 0" class="client__state">
            {{ t("module.home.client.empty") }}
          </p>
          <ul v-else class="client__library">
            <li
              v-for="client in clients"
              :key="client.id"
              :class="[
                'client__library-item',
                { 'client__library-item--active': current?.client_id === client.id },
              ]"
            >
              <button
                type="button"
                class="client__library-row"
                :disabled="busy || !client.available"
                @click="assignClient(client)"
              >
                <span class="client__library-icon"><PlugZap :size="18" :stroke-width="1.7" /></span>
                <span class="client__library-main">
                  <span class="client__library-name">{{ client.name }}</span>
                  <span class="client__library-meta">
                    {{ client.file_name }} · {{ formatBytes(client.size) }}
                    <template v-if="client.imported_at"> · {{ formatDate(client.imported_at) }}</template>
                  </span>
                </span>
                <span v-if="!client.available" class="client__missing">
                  <FileWarning :size="14" />
                  {{ t("module.home.client.missing") }}
                </span>
                <Check
                  v-else-if="current?.client_id === client.id"
                  :size="18"
                  class="client__check"
                />
              </button>
              <button
                type="button"
                class="client__library-delete"
                :title="t('module.home.client.delete')"
                :aria-label="t('module.home.client.delete')"
                :disabled="busy"
                @click.stop="removeClient(client)"
              >
                <Trash2 :size="15" />
              </button>
            </li>
          </ul>

          <div v-if="importOpen" class="client__import">
            <h3 class="client__import-title">{{ t("module.home.client.import_title") }}</h3>
            <p class="client__import-path">{{ importPath }}</p>
            <label class="client__field">
              <span class="client__label">{{ t("module.home.client.name_label") }}</span>
              <CoTextField
                :model-value="importName"
                :placeholder="t('module.home.client.name_placeholder')"
                :disabled="importing"
                @update:model-value="importName = $event"
                @enter="confirmImport"
              />
              <span class="client__field-hint">{{ t("module.home.client.name_hint") }}</span>
            </label>
            <p v-if="importError" class="client__error">{{ importError }}</p>
            <div class="client__import-actions">
              <CoButton variant="secondary" size="sm" :disabled="importing" @click="cancelImport">
                {{ t("module.home.client.cancel") }}
              </CoButton>
              <CoButton variant="primary" size="sm" :disabled="importing" @click="confirmImport">
                <span>{{ t("module.home.client.import_confirm") }}</span>
              </CoButton>
            </div>
          </div>
        </template>
      </template>

      <template v-else>
        <div v-if="!launchState" class="client__state">
          {{ t("module.home.client.launch_unavailable") }}
        </div>
        <ul v-else class="client__rows">
          <li class="client__row">
            <div class="client__row-text">
              <span class="client__row-title">{{ t("module.home.client.launch_injected") }}</span>
              <span class="client__row-hint">
                {{
                  launchState.injected
                    ? t("module.home.client.launch_injected_hint_on")
                    : t("module.home.client.launch_injected_hint_off")
                }}
              </span>
            </div>
            <span class="client__row-value">
              {{
                launchState.injected
                  ? t("module.home.client.launch_value_on")
                  : t("module.home.client.launch_value_off")
              }}
            </span>
          </li>
          <li class="client__row">
            <div class="client__row-text">
              <span class="client__row-title">{{ t("module.home.client.launch_backup") }}</span>
            </div>
            <span class="client__row-value">{{ backupLabel }}</span>
          </li>
          <li class="client__row">
            <div class="client__row-text">
              <span class="client__row-title">{{ t("module.home.client.launch_preload") }}</span>
            </div>
            <span class="client__row-value">{{ preloadLabel || t("common.unknown") }}</span>
          </li>
        </ul>
      </template>
    </div>

    <template #aside>
      <InstanceFilterList v-model:active="category" :items="categories" />
      <div v-if="category === 'dll' && isWindows" class="client__actions">
        <CoButton variant="secondary" :disabled="busy || !current" @click="pickFile">
          <Upload :size="14" />
          <span>{{ t("module.home.client.import") }}</span>
        </CoButton>
        <CoButton
          v-if="current?.client_id"
          variant="ghost"
          :disabled="busy"
          @click="clearClient"
        >
          {{ t("module.home.client.clear") }}
        </CoButton>
      </div>
      <p v-else-if="category === 'dll'" class="client__aside-hint">
        {{ t("module.home.client.windows_only") }}
      </p>
      <p v-else class="client__aside-hint">
        {{ t("module.home.client.launch_unavailable") }}
      </p>
    </template>
  </InstanceManageShell>
</template>

<style scoped>
.client {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  padding: var(--copper-space-4);
}

.client__hint {
  margin: 0;
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
  line-height: 1.5;
  max-width: 62ch;
}

.client__current {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
  padding: var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
}

.client__current-label {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.client__current-name {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
  overflow-wrap: anywhere;
}

.client__current-name--empty {
  color: var(--copper-text-disabled);
  font-weight: 400;
}

.client__badge {
  align-self: flex-start;
  margin-top: var(--copper-space-1);
  padding: 2px 8px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-3);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.client__badge--on {
  background: color-mix(in srgb, var(--copper-accent) 18%, transparent);
  color: var(--copper-accent);
}

.client__warn {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-2) var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  border: 1px solid color-mix(in srgb, var(--copper-warning) 45%, var(--copper-border));
  background: color-mix(in srgb, var(--copper-warning) 10%, transparent);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  line-height: 1.5;
}

.client__list-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding-top: var(--copper-space-1);
}

.client__list-title {
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

.client__state {
  padding: var(--copper-space-4) 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  text-align: center;
}

.client__library {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0;
  padding: 0;
}

.client__library-item {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
  border-radius: var(--copper-radius-md);
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.client__library-item:hover {
  background: var(--copper-hover);
}

.client__library-item--active {
  background: color-mix(in srgb, var(--copper-accent) 12%, transparent);
}

.client__library-row {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-2);
  border: none;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
}

.client__library-row:disabled {
  cursor: not-allowed;
  opacity: 0.6;
}

.client__library-icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 36px;
  height: 36px;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
}

.client__library-item--active .client__library-icon {
  background: color-mix(in srgb, var(--copper-accent) 18%, transparent);
  color: var(--copper-accent);
}

.client__library-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.client__library-name {
  overflow: hidden;
  font-size: var(--copper-font-size-md);
  font-weight: 500;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.client__library-meta {
  overflow: hidden;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
  text-overflow: ellipsis;
}

.client__check {
  flex-shrink: 0;
  color: var(--copper-accent);
}

.client__missing {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--copper-danger);
  font-size: var(--copper-font-size-xs);
}

.client__library-delete {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
}

.client__library-delete:hover:not(:disabled) {
  background: color-mix(in srgb, var(--copper-danger) 14%, transparent);
  color: var(--copper-danger);
}

.client__library-delete:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.client__import {
  margin-top: var(--copper-space-2);
  padding: var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
}

.client__import-title {
  margin: 0 0 var(--copper-space-1);
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

.client__import-path {
  margin: 0 0 var(--copper-space-3);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  overflow-wrap: anywhere;
}

.client__field {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.client__label {
  font-size: var(--copper-font-size-sm);
}

.client__field-hint {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.client__error {
  margin: var(--copper-space-2) 0 0;
  color: var(--copper-danger);
  font-size: var(--copper-font-size-xs);
}

.client__import-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--copper-space-2);
  margin-top: var(--copper-space-3);
}

.client__rows {
  list-style: none;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--copper-border);
}

.client__row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) 0;
  border-bottom: 1px solid var(--copper-border);
}

.client__row-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.client__row-title {
  font-size: var(--copper-font-size-md);
  font-weight: 500;
}

.client__row-hint {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.client__row-value {
  flex-shrink: 0;
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
  text-align: right;
}

.client__actions {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding-top: var(--copper-space-2);
  margin-top: var(--copper-space-1);
  border-top: 1px solid var(--copper-border);
}

.client__aside-hint {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
  line-height: 1.5;
}
</style>