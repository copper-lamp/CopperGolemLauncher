<script setup lang="ts">
// 存储占用总览与清理二级页。游戏目录仍写入 game.directory，由内核统一解析。
//
// 用量扫描完全交给后端后台线程：本页只订阅快照与事件，进入页面时投递一次扫描，
// 分类结果谁先完成谁先显示，页面本身从不等待整轮扫盘。

import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";

import StorageUsageCard from "../../components/ui/StorageUsageCard.vue";
import StorageCleanupPanel from "../../components/ui/StorageCleanupPanel.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { homeVersionsRoot } from "../../api/home";
import { storageStatus } from "../../api/storage";
import { useStorageScan } from "../../composables/useStorageScan";
import { usePlatform } from "../../composables/usePlatform";
import { formatBytes } from "../../api/download";
import CoButton from "../../components/ui/CoButton.vue";

const { t } = useI18n();
const { get, set } = useSettings();

const resolvedRoot = ref("");
const environmentOverride = ref(false);
const cleanupView = ref(false);
const { isAndroid } = usePlatform();
const { categories, scanning, init: initScan, rescan, clean: cleanScan } = useStorageScan();

const gameDir = computed({
  get: () => get<string>("game.directory", ""),
  set: (value: string) => void set("game.directory", value),
});

// 目录解析与设置状态和扫盘无关，单独加载，不参与用量卡片的加载态。
async function loadRootState() {
  if (isAndroid.value) return;
  try {
    resolvedRoot.value = await homeVersionsRoot();
    const status = await storageStatus();
    environmentOverride.value = status.environmentOverride;
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function pickFolder() {
  const selected = await open({ directory: true, multiple: false });
  if (typeof selected === "string" && selected) gameDir.value = selected;
}

function resetDir() {
  gameDir.value = "";
  void loadRootState();
  void rescan(true);
}

async function clean(categoriesToClean: string[]) {
  try {
    const freed = await cleanScan(categoriesToClean);
    showToast(t("settings.storage.cleanup.success", { size: formatBytes(freed) }), "success");
  } catch (e) {
    showToast(String(e), "error");
  }
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
watch(isAndroid, () => {
  if (!isAndroid.value) void loadRootState();
});
watch(gameDir, () => {
  if (refreshTimer !== undefined) clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => {
    void loadRootState();
    void rescan(true);
  }, 300);
});
onBeforeUnmount(() => {
  if (refreshTimer !== undefined) clearTimeout(refreshTimer);
});

onMounted(() => {
  void loadRootState();
  void initScan();
});
</script>

<template>
  <div class="storage-tab">
    <template v-if="!cleanupView">
      <StorageUsageCard />
      <div class="storage-tab__actions">
        <CoButton variant="primary" @click="cleanupView = true">{{ t("settings.storage.cleanup.title") }}</CoButton>
      </div>
    </template>
    <StorageCleanupPanel
      v-else
      :categories="categories"
      :scanning="scanning"
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
.storage-tab{max-width:760px}
.storage-tab__actions{display:flex;justify-content:flex-end;margin-top:var(--copper-space-3)}
</style>
