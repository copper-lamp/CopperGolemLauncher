<script setup lang="ts">
// 存储占用总览与清理二级页。游戏目录仍写入 game.directory，由内核统一解析。

import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";

import StorageUsageCard from "../../components/ui/StorageUsageCard.vue";
import StorageCleanupPanel from "../../components/ui/StorageCleanupPanel.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { homeVersionsRoot } from "../../api/home";
import { storageStatus, storageUsage, storageClean, type StorageUsage } from "../../api/storage";
import { usePlatform } from "../../composables/usePlatform";
import { formatBytes } from "../../api/download";
import CoButton from "../../components/ui/CoButton.vue";

const { t } = useI18n();
const { get, set } = useSettings();

const resolvedRoot = ref("");
const environmentOverride = ref(false);
const usage = ref<StorageUsage | null>(null);
const loading = ref(true);
const cleanupView = ref(false);
const { isAndroid } = usePlatform();

const gameDir = computed({
  get: () => get<string>("game.directory", ""),
  set: (value: string) => void set("game.directory", value),
});

async function refresh() {
  loading.value = true;
  const [usageResult, rootResult] = await Promise.allSettled([
    storageUsage(),
    isAndroid.value ? Promise.resolve("") : homeVersionsRoot(),
  ]);
  if (usageResult.status === "fulfilled") usage.value = usageResult.value;
  else showToast(String(usageResult.reason), "error");
  if (!isAndroid.value) {
    if (rootResult.status === "fulfilled") resolvedRoot.value = rootResult.value;
    try {
      const status = await storageStatus();
      environmentOverride.value = status.environmentOverride;
    } catch (e) { showToast(String(e), "error"); }
  }
  loading.value = false;
}

async function pickFolder() {
  const selected = await open({ directory: true, multiple: false });
  if (typeof selected === "string" && selected) gameDir.value = selected;
}

function resetDir() {
  gameDir.value = "";
  void refresh();
}

async function clean(categories: string[]) {
  try {
    const result = await storageClean(categories);
    usage.value = result.usage;
    showToast(t("settings.storage.cleanup.success", { size: formatBytes(result.freedBytes) }), "success");
  } catch (e) { showToast(String(e), "error"); }
}

async function reveal(path: string) {
  if (!path) return;
  try {
    await openPath(path);
  } catch (e) {
    showToast(String(e), "error");
  }
}

// 逐字符输入时不需要每次扫盘；去抖后只刷新一次生效路径。
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
watch(isAndroid, () => { if (!isAndroid.value) void refresh(); });
watch(gameDir, () => {
  if (refreshTimer !== undefined) clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => void refresh(), 300);
});
onBeforeUnmount(() => {
  if (refreshTimer !== undefined) clearTimeout(refreshTimer);
});

onMounted(refresh);
</script>

<template>
  <div class="storage-tab">
    <template v-if="!cleanupView">
      <StorageUsageCard :usage="usage" :loading="loading" />
      <div class="storage-tab__actions"><CoButton variant="primary" @click="cleanupView = true">{{ t("settings.storage.cleanup.title") }}</CoButton></div>
    </template>
    <StorageCleanupPanel
      v-else
      :usage="usage"
      :resolved-root="resolvedRoot"
      :game-dir="gameDir"
      :environment-override="environmentOverride"
      :is-android="isAndroid"
      @back="cleanupView = false"
      @clean="clean"
      @update:game-dir="gameDir = $event"
      @browse="pickFolder"
      @reset="resetDir"
      @open="reveal(resolvedRoot)"
    />
  </div>
</template>

<style scoped>
.storage-tab {
  max-width: 760px;
}

.storage-tab__actions{display:flex;justify-content:flex-end;margin-top:var(--copper-space-3)}
</style>
