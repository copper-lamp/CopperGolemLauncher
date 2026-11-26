<script setup lang="ts">
// 储存位置（一级页）：游戏数据目录 + 存储用量图表。
//
// 清理是独立子路由 settings-storage-cleanup，面包屑因此能拼出
// 「设置 → 储存位置 → 清理储存」；本页只负责目录设置与图表，不阻塞扫描。
//
// 用量扫描完全交给后端后台线程：本页只订阅快照与事件，进入页面时投递一次扫描，
// 分类结果谁先完成谁先显示，页面本身从不等待整轮扫盘。

import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { Trash2 } from "@lucide/vue";

import StorageUsageCard from "../../components/ui/StorageUsageCard.vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { homeVersionsRoot } from "../../api/home";
import { storageStatus } from "../../api/storage";
import { useStorageScan } from "../../composables/useStorageScan";
import { usePlatform } from "../../composables/usePlatform";

const { t } = useI18n();
const { get, set } = useSettings();
const router = useRouter();

const resolvedRoot = ref("");
const environmentOverride = ref(false);
const { isAndroid } = usePlatform();
const { init: initScan } = useStorageScan();

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
}

async function reveal(path: string) {
  if (!path) return;
  try {
    await openPath(path);
  } catch (e) {
    showToast(String(e), "error");
  }
}

function openCleanup() {
  void router.push({ name: "settings-storage-cleanup" });
}

// 用量只统计缓存与日志，不包含游戏本体与存档，所以目录变化只需要刷新解析结果。
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
watch(isAndroid, () => {
  if (!isAndroid.value) void loadRootState();
});
watch(gameDir, () => {
  if (refreshTimer !== undefined) clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => void loadRootState(), 300);
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
    <SettingSection v-if="!isAndroid" title-key="settings.storage.game_dir">
      <SettingRow label-key="settings.storage.game_dir" hint-key="settings.storage.game_dir_hint">
        <div class="game-dir__row">
          <CoTextField
            class="game-dir__field"
            :model-value="gameDir"
            :placeholder="t('settings.storage.game_dir_default')"
            :disabled="environmentOverride"
            @update:model-value="gameDir = $event"
          />
          <CoButton variant="secondary" size="sm" :disabled="environmentOverride" @click="pickFolder"><span>{{ t("settings.storage.browse") }}</span></CoButton>
          <CoButton variant="ghost" size="sm" :disabled="environmentOverride" @click="resetDir"><span>{{ t("settings.storage.reset") }}</span></CoButton>
          <CoButton variant="secondary" size="sm" @click="reveal(resolvedRoot)"><span>{{ t("settings.storage.open") }}</span></CoButton>
        </div>
      </SettingRow>
      <p class="game-dir__resolved">{{ t("settings.storage.resolved", { path: resolvedRoot || "—" }) }}</p>
      <p v-if="(gameDir.trim() && resolvedRoot !== gameDir.trim()) || environmentOverride" class="game-dir__warn">{{ t("settings.storage.overridden") }}</p>
    </SettingSection>

    <StorageUsageCard />

    <div class="storage-tab__footer">
      <p class="storage-tab__note">{{ t("settings.storage.cleanup.lead") }}</p>
      <CoButton variant="primary" @click="openCleanup">
        <Trash2 :size="16" /><span>{{ t("settings.storage.cleanup.title") }}</span>
      </CoButton>
    </div>
  </div>
</template>

<style scoped>
.storage-tab{display:grid;gap:var(--copper-space-4);max-width:840px}
.game-dir__row{display:flex;align-items:center;gap:var(--copper-space-2);flex-wrap:wrap}
.game-dir__field{flex:1 1 240px;min-width:180px}
.game-dir__resolved,.game-dir__warn{margin:0 var(--copper-space-2) var(--copper-space-2);font-size:var(--copper-font-size-xs);overflow-wrap:anywhere}
.game-dir__resolved{color:var(--copper-text-secondary)}
.game-dir__warn{color:var(--copper-warning)}
.storage-tab__footer{display:flex;align-items:center;justify-content:space-between;gap:var(--copper-space-3);flex-wrap:wrap}
.storage-tab__note{margin:0;font-size:var(--copper-font-size-sm);color:var(--copper-text-secondary)}
</style>
