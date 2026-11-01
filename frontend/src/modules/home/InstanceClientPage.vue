<script setup lang="ts">
// 实例 · 客户端页（路由 `/version-settings/client`）：左侧是客户端 DLL 与启动状态，
// 右侧副栏放操作（选择 / 更换 / 清除）。
//
// 只做两件事：
// 1. 选择 / 清除 DLL —— 落 `home_version_save_meta({ client_dll })`，后端负责去空白
//    与"切出客户端模式时清空"，本页不做本地猜测，一律以返回视图为准；
// 2. 展示启动文件与预加载状态 —— 复用 `home_launch_file_state` / `home_preload_summary`，
//    不新增后端命令。注入是否真的生效由启动链路决定（见 docs/启动链路与实例隔离.md），
//    这里只汇报事实，不承诺"已可用"。

import { computed, ref, watch } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { PlugZap, Rocket } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoButton from "../../components/ui/CoButton.vue";
import InstanceFilterList from "./InstanceFilterList.vue";
import InstanceManageShell from "./InstanceManageShell.vue";
import {
  homeLaunchFileState,
  homePreloadSummary,
  homeVersionSaveMeta,
  type LaunchFileState,
  type PreloadSummary,
} from "../../api/home";
import { useVersionInstance } from "./useVersionInstance";

const { t } = useI18n();
const { current, currentName, applyView } = useVersionInstance();

const category = ref("dll");
const busy = ref(false);
const launchState = ref<LaunchFileState | null>(null);
const preload = ref<PreloadSummary | null>(null);

const categories = computed(() => [
  { value: "dll", label: t("module.home.client.category.dll"), icon: PlugZap },
  { value: "launch", label: t("module.home.client.category.launch"), icon: Rocket },
]);

const title = computed(() =>
  category.value === "dll"
    ? t("module.home.client.category.dll")
    : t("module.home.client.category.launch"),
);

/** 备份大小的人类可读文本（与入口页 / 基本设置同一套口径）。 */
function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 MB";
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

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

watch(currentName, () => void refreshLaunchState(), { immediate: true });

async function pickDll() {
  const view = current.value;
  if (!view) return;
  const picked = await openDialog({
    multiple: false,
    filters: [{ name: t("module.home.client.dll_filter"), extensions: ["dll"] }],
  });
  if (typeof picked !== "string" || !picked) return;
  busy.value = true;
  try {
    const updated = await homeVersionSaveMeta(view.name, { client_dll: picked });
    applyView(updated);
    showToast(t("module.home.client.saved"), "success");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}

async function clearDll() {
  const view = current.value;
  if (!view) return;
  busy.value = true;
  try {
    const updated = await homeVersionSaveMeta(view.name, { client_dll: "" });
    applyView(updated);
    showToast(t("module.home.client.cleared"), "success");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <InstanceManageShell :title="title">
    <div class="client">
      <template v-if="category === 'dll'">
        <p class="client__hint">{{ t("module.home.client.hint") }}</p>

        <div class="client__current">
          <span class="client__current-label">{{ t("module.home.client.current") }}</span>
          <span :class="['client__path', { 'client__path--empty': !current?.client_dll }]">
            {{ current?.client_dll ?? t("module.home.client.none") }}
          </span>
        </div>

        <p v-if="current && current.mode !== 'client'" class="client__warn">
          {{ t("module.home.client.not_client_mode") }}
        </p>
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
      <div v-if="category === 'dll'" class="client__actions">
        <CoButton variant="secondary" :disabled="busy || !current" @click="pickDll">
          <PlugZap :size="14" />
          <span>
            {{ current?.client_dll ? t("module.home.client.change") : t("module.home.client.pick") }}
          </span>
        </CoButton>
        <CoButton
          v-if="current?.client_dll"
          variant="ghost"
          :disabled="busy"
          @click="clearDll"
        >
          {{ t("module.home.client.clear") }}
        </CoButton>
      </div>
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
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
}

.client__current-label {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.client__path {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text);
  overflow-wrap: anywhere;
}

.client__path--empty {
  color: var(--copper-text-disabled);
}

/* 提示条：不是错误，而是"这样设置不会生效"的前置条件提醒。 */
.client__warn {
  padding: var(--copper-space-2) var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  border: 1px solid color-mix(in srgb, var(--copper-warning) 45%, var(--copper-border));
  background: color-mix(in srgb, var(--copper-warning) 10%, transparent);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  line-height: 1.5;
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

.client__state {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
}
</style>
