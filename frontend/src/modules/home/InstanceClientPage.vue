<script setup lang="ts">
// 实例 · 客户端页（路由 /version-settings/client）。
//
// 版面：左栏是「当前客户端 / 本地客户端」两段纯列表，右栏是独立搜索框 +
// 客户端面板。没有卡片包裹，也不用描边分层 —— 分组靠留白与色阶。
//
// 点击本地列表里的客户端 = 把它设为当前实例的客户端，并同时把实例切到
// 客户端模式；「客户端模式」不是独立开关，避免出现「选了客户端但不生效」。
// 客户端文件由启动器数据目录统一保管，实例只记 client_id。

import { computed, onMounted, ref } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Boxes, Check, PlugZap } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { usePlatform } from "../../composables/usePlatform";
import InstanceSearchBox from "./InstanceSearchBox.vue";
import ClientPanel from "./ClientPanel.vue";
import {
  homeClientDelete,
  homeClientImport,
  homeClientsList,
  homeClientSetInjectDelay,
  homeVersionSaveMeta,
  type ClientView,
} from "../../api/home";
import { useVersionInstance } from "./useVersionInstance";
import { useCompatibilityText } from '../../services/compatibility-messages';

const { t } = useI18n();
const { isWindows } = usePlatform();
const { current, applyView } = useVersionInstance();
const compatText = useCompatibilityText();

const busy = ref(false);
const clients = ref<ClientView[]>([]);
const loading = ref(true);
const query = ref("");

/** 已选好文件、等待命名的导入表单。 */
const importPath = ref("");
const importName = ref("");
const importError = ref("");
const importing = ref(false);
const delaySaving = ref(false);

const clientMode = computed(() => current.value?.mode === "client");

const selectedClient = computed(
  () => clients.value.find((client) => client.id === current.value?.client_id) ?? null,
);

const filteredClients = computed(() => {
  const keyword = query.value.trim().toLowerCase();
  if (!keyword) return clients.value;
  return clients.value.filter((client) => client.name.toLowerCase().includes(keyword));
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
    loading.value = false;
    return;
  }
  loading.value = true;
  try {
    clients.value = await homeClientsList();
  } catch (e) {
    clients.value = [];
    showToast(String(e), "error");
  } finally {
    loading.value = false;
  }
}

async function saveMeta(update: { mode?: string; client_id?: string }) {
  const version = current.value;
  if (!version || (version.compatibility && !version.compatibility.capabilities.client)) return false;
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

/** 点击本地客户端：设为当前客户端，并确保实例处于客户端模式。 */
async function assignClient(client: ClientView) {
  if (busy.value) return;
  const version = current.value;
  if (!version) return;
  const alreadySelected = version.client_id === client.id;
  if (alreadySelected && clientMode.value) return;
  if (!client.available) {
    showToast(t("module.home.client.missing"), "error");
    return;
  }
  const update: { mode?: string; client_id?: string } = { client_id: client.id };
  if (!alreadySelected || !clientMode.value) update.mode = "client";
  if (await saveMeta(update)) {
    showToast(t("module.home.client.selected_saved"), "success");
  }
}

async function enableClientMode() {
  if (!selectedClient.value) return;
  if (await saveMeta({ mode: "client" })) {
    showToast(t("module.home.client.mode_on"), "success");
  }
}

function startImport(path: string) {
  const fileName = path.split(/[\\/]/).pop() ?? "client.dll";
  importPath.value = path;
  importName.value = fileName.replace(/\.dll$/i, "");
  importError.value = "";
}

async function pickFile() {
  if (busy.value || importing.value) return;
  if (!isWindows.value) {
    showToast(t("module.home.client.windows_only"), "error");
    return;
  }
  const picked = await openDialog({
    multiple: false,
    filters: [{ name: t("module.home.client.dll_filter"), extensions: ["dll"] }],
  });
  if (typeof picked !== "string" || !picked) return;
  startImport(picked);
}

function droppedFile(path: string) {
  if (!isWindows.value) {
    showToast(t("module.home.client.windows_only"), "error");
    return;
  }
  startImport(path);
}

function rejectDrop() {
  showToast(t("module.home.client.drop_not_dll"), "error");
}

function cancelImport() {
  if (importing.value) return;
  importPath.value = "";
  importName.value = "";
  importError.value = "";
}

async function confirmImport() {
  if (importing.value || !importPath.value) return;
  importing.value = true;
  importError.value = "";
  try {
    const view = await homeClientImport(importPath.value, importName.value);
    importPath.value = "";
    importName.value = "";
    await loadLibrary();
    // 当前实例还没选客户端时自动选上新导入的这个；已选则保持不动。
    if (!current.value?.client_id) {
      await saveMeta({ mode: "client", client_id: view.id });
    }
    showToast(t("module.home.client.imported"), "success");
  } catch (e) {
    importError.value = String(e);
  } finally {
    importing.value = false;
  }
}

async function removeSelected() {
  const client = selectedClient.value;
  if (!client || busy.value) return;
  if (!window.confirm(t("module.home.client.delete_confirm", { name: client.name }))) return;
  busy.value = true;
  try {
    await homeClientDelete(client.id);
    // 删掉的正好是本实例选中的客户端：清掉引用，避免下次启动找不到记录。
    if (current.value?.client_id === client.id) {
      applyView(await homeVersionSaveMeta(current.value.name, { client_id: "" }));
    }
    await loadLibrary();
    showToast(t("module.home.client.deleted"), "success");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}

/** 注入时机按客户端保存（秒 → 毫秒），后端会再校验一次范围。 */
async function saveDelay(seconds: number) {
  const client = selectedClient.value;
  if (!client) return;
  delaySaving.value = true;
  try {
    const updated = await homeClientSetInjectDelay(client.id, seconds * 1000);
    clients.value = clients.value.map((item) => (item.id === updated.id ? updated : item));
    showToast(t("module.home.client.inject_saved"), "success");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    delaySaving.value = false;
  }
}

onMounted(loadLibrary);
</script>

<template>
  <p v-if="current?.compatibility && !current.compatibility.capabilities.client" role="status">{{ compatText('unsupported') }}</p>
  <div v-else class="client-page">
    <section class="client-page__main">
      <div class="client-page__body">
        <section class="group">
          <header class="group__head">
            <PlugZap :size="14" :stroke-width="1.8" />
            <span class="group__title">{{ t("module.home.client.current") }}</span>
            <span v-if="selectedClient" class="group__count">{{ selectedClient.name }}</span>
          </header>

          <div v-if="selectedClient" class="current" :class="{ 'current--off': !clientMode }">
            <span class="row__icon">
              <PlugZap :size="17" :stroke-width="1.7" />
            </span>
            <span class="row__text">
              <span class="row__name">{{ selectedClient.name }}</span>
              <span class="row__meta">
                {{ selectedClient.file_name }} · {{ formatBytes(selectedClient.size) }}
                <template v-if="selectedClient.imported_at"> · {{ formatDate(selectedClient.imported_at) }}</template>
              </span>
            </span>
            <span v-if="!selectedClient.available" class="row__flag row__flag--warn">
              {{ t("module.home.client.missing") }}
            </span>
            <span v-else class="current__badge">
              <Check :size="12" />
              {{ clientMode ? t("module.home.client.mode_on_short") : t("module.home.client.mode_off_short") }}
            </span>
          </div>

          <p v-else class="state" :class="{ 'state--hint': !loading }">
            {{ loading ? "..." : clientMode ? t("module.home.client.mode_no_client") : t("module.home.client.none") }}
          </p>
        </section>

        <section class="group">
          <header class="group__head">
            <Boxes :size="14" :stroke-width="1.8" />
            <span class="group__title">{{ t("module.home.client.library") }}</span>
            <span class="group__count">{{ filteredClients.length }}</span>
          </header>
          <p v-if="!isWindows" class="state state--hint">{{ t("module.home.client.windows_only") }}</p>
          <p v-else-if="!loading && !clients.length" class="state state--hint">
            {{ t("module.home.client.empty") }}
          </p>
          <p v-else-if="!loading && !filteredClients.length" class="state state--hint">
            {{ t("module.home.client.no_match") }}
          </p>
          <div v-else class="group__list">
            <button
              v-for="client in filteredClients"
              :key="client.id"
              type="button"
              class="row"
              :class="{
                'row--active': client.id === current?.client_id,
                'row--missing': !client.available,
              }"
              :disabled="busy"
              @click="assignClient(client)"
            >
              <span class="row__icon">
                <PlugZap :size="17" :stroke-width="1.7" />
              </span>
              <span class="row__text">
                <span class="row__name">{{ client.name }}</span>
                <span class="row__meta">
                  {{ client.file_name }} · {{ formatBytes(client.size) }}
                  <template v-if="client.imported_at"> · {{ formatDate(client.imported_at) }}</template>
                </span>
              </span>
              <span v-if="!client.available" class="row__flag row__flag--warn">
                {{ t("module.home.client.missing") }}
              </span>
              <span v-else-if="client.id === current?.client_id" class="row__flag">
                <Check :size="14" />
              </span>
            </button>
          </div>
        </section>
      </div>
    </section>

    <aside class="client-page__aside">
      <div class="client-page__search">
        <InstanceSearchBox v-model="query" :placeholder="t('module.home.client.search_placeholder')" />
      </div>
      <ClientPanel
        :client="selectedClient"
        :client-mode="clientMode"
        :importing="importing"
        :saving="delaySaving || busy"
        :import-path="importPath"
        :import-name="importName"
        :import-error="importError"
        @pick-file="pickFile"
        @import-dropped="droppedFile"
        @reject-drop="rejectDrop"
        @confirm-import="confirmImport"
        @cancel-import="cancelImport"
        @update:import-name="importName = $event"
        @delete="removeSelected"
        @enable-mode="enableClientMode"
        @set-delay="saveDelay"
      />
    </aside>
  </div>
</template>

<style scoped>
.client-page {
  display: grid;
  grid-template-columns: 6fr 4fr;
  gap: var(--copper-space-4);
  height: 100%;
  min-height: 0;
  padding: var(--copper-space-4) var(--copper-space-4) var(--copper-space-3);
}

.client-page__main {
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.client-page__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-5);
  padding-bottom: var(--copper-space-3);
}

.client-page__aside {
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
}

.client-page__search {
  flex-shrink: 0;
}

.group {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.group__head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  padding: 0 var(--copper-space-2);
  color: var(--copper-text-secondary);
}

.group__title {
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

.group__count {
  overflow: hidden;
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
  text-overflow: ellipsis;
}

.group__list {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

/* 列表行：整行可点，无描边；悬停是薄雾，选中是微色阶 + 投影。 */
.row {
  width: 100%;
  min-height: 52px;
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  padding: 0 var(--copper-space-2);
  border: none;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text);
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.row:hover {
  background: var(--copper-hover);
}

.row--active {
  background: color-mix(in srgb, var(--copper-accent) 12%, transparent);
  box-shadow: var(--copper-shadow-raised);
}

.row--active:hover {
  background: color-mix(in srgb, var(--copper-accent) 16%, transparent);
}

.row--missing {
  color: var(--copper-text-disabled);
}

.row:disabled {
  cursor: default;
}

.row__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border-radius: var(--copper-radius-sm);
  background: color-mix(in srgb, var(--copper-text) 6%, transparent);
  color: var(--copper-text-secondary);
}

.row--active .row__icon {
  background: color-mix(in srgb, var(--copper-accent) 20%, transparent);
  color: var(--copper-accent);
}

.row__text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.row__name {
  overflow: hidden;
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.row__meta {
  overflow: hidden;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
  text-overflow: ellipsis;
}

.row__flag {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 2px;
  color: var(--copper-accent);
  font-size: var(--copper-font-size-xs);
}

.row__flag--warn {
  color: var(--copper-danger);
}

.current {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  min-height: 56px;
  padding: 0 var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  background: color-mix(in srgb, var(--copper-accent) 12%, transparent);
  box-shadow: var(--copper-shadow-raised);
}

.current--off {
  background: var(--copper-surface-2);
  box-shadow: none;
}

.current__badge {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 2px 8px;
  border-radius: var(--copper-radius-full);
  background: color-mix(in srgb, var(--copper-accent) 18%, transparent);
  color: var(--copper-accent);
  font-size: var(--copper-font-size-xs);
}

.state {
  margin: 0;
  padding: var(--copper-space-4);
  border-radius: var(--copper-radius-md);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  line-height: 1.6;
  text-align: center;
}

.state--hint {
  background: var(--copper-surface-2);
  color: var(--copper-text-disabled);
}

@media (max-width: 900px) {
  .client-page {
    grid-template-columns: 1fr;
    overflow-y: auto;
  }

  .client-page__main,
  .client-page__body,
  .client-page__aside {
    min-height: auto;
  }

  .client-page__body {
    overflow: visible;
  }
}
</style>
