<script setup lang="ts">
// 开始页 · 简洁模式：页面中央竖直排版 文案 → 版本名 → 启动按钮 → 版本设置。
// 左上角为 简洁 / 默认 模式切换；默认模式（Win10 磁贴风格）本次不实现，展示占位。
//
// 多个版本时版本名以选择器呈现（核心功能「选择版本」），否则仅显示版本名。

import { computed, onMounted, onUnmounted, ref } from "vue";
import { useRouter } from "vue-router";
import { Play, Gamepad2, Download } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoButton from "../../components/ui/CoButton.vue";
import CoSegmented from "../../components/ui/CoSegmented.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import {
  homeLaunch,
  homeVersionsList,
  type VersionView,
} from "../../api/home";
import { onVersionInstalled, onVersionRemoved } from "../../events";

type HomeMode = "simple" | "default";

const { t } = useI18n();
const router = useRouter();

const mode = ref<HomeMode>("simple");
const versions = ref<VersionView[]>([]);
const selectedName = ref("");
const launching = ref(false);
const loading = ref(true);

const modeOptions = computed(() => [
  { value: "simple", label: t("module.home.mode.simple") },
  { value: "default", label: t("module.home.mode.default") },
]);

/** 当前版本（多版本时可经选择器切换）。 */
const current = computed(() => {
  if (!selectedName.value) return null;
  return (
    versions.value.find((v) => v.name === selectedName.value) ?? null
  );
});

const versionOptions = computed(() =>
  versions.value.map((v) => ({ value: v.name, label: v.name })),
);

onMounted(async () => {
  await refresh();
  // 版本安装 / 删除事件直达刷新，无需重启。
  unlisten = [
    await onVersionInstalled(() => void refresh()),
    await onVersionRemoved(() => void refresh()),
  ];
});

let unlisten: Array<() => void> = [];

onUnmounted(() => {
  unlisten.forEach((u) => u());
  unlisten = [];
});

async function refresh() {
  try {
    versions.value = await homeVersionsList();
    if (!versions.value.some((v) => v.name === selectedName.value)) {
      selectedName.value = versions.value[0]?.name ?? "";
    }
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    loading.value = false;
  }
}

async function launch() {
  if (!current.value || launching.value) return;
  launching.value = true;
  try {
    await homeLaunch(current.value.name);
    showToast(t("module.home.toast.launch_started"), "success");
  } catch (e) {
    showToast(t("module.home.toast.launch_failed", { message: String(e) }), "error");
  } finally {
    launching.value = false;
  }
}

function openVersionSettings() {
  if (!current.value) return;
  void router.push({
    path: "/version-settings",
    query: { name: current.value.name },
  });
}

function goDownload() {
  void router.push("/game-download");
}
</script>

<template>
  <div class="home-page">
    <div class="home-page__mode">
      <CoSegmented v-model="mode" :options="modeOptions" />
    </div>

    <!-- 默认模式占位（后续阶段实现） -->
    <div v-if="mode === 'default'" class="home-page__default-placeholder">
      <Gamepad2 :size="40" :stroke-width="1.5" />
      <p>{{ t("module.home.mode.default_placeholder") }}</p>
    </div>

    <!-- 简洁模式 -->
    <div v-else class="home-page__simple">
      <template v-if="loading">
        <p class="home-page__hint">{{ t("common.loading") }}</p>
      </template>

      <template v-else-if="!current">
        <h1 class="home-page__hero">{{ t("module.home.hero_title") }}</h1>
        <p class="home-page__empty">{{ t("module.home.empty") }}</p>
        <p class="home-page__hint">{{ t("module.home.install_hint") }}</p>
        <div class="home-page__actions">
          <CoButton variant="primary" size="md" @click="goDownload">
            <Download :size="16" />
            <span>{{ t("module.home.go_download") }}</span>
          </CoButton>
        </div>
      </template>

      <template v-else>
        <h1 class="home-page__hero">{{ t("module.home.hero_title") }}</h1>
        <div class="home-page__version">
          <CoSelect
            v-if="versionOptions.length > 1"
            v-model="selectedName"
            :options="versionOptions"
            class="home-page__version-select"
          />
          <span v-else class="home-page__version-name">{{ current.name }}</span>
        </div>
        <div class="home-page__actions">
          <CoButton variant="primary" size="md" :disabled="launching" @click="launch">
            <Play :size="16" />
            <span>{{ launching ? t("module.home.playing") : t("module.home.play") }}</span>
          </CoButton>
          <CoButton variant="ghost" size="md" @click="openVersionSettings">
            {{ t("module.home.settings") }}
          </CoButton>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.home-page {
  position: relative;
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  overflow-y: auto;
}

.home-page__mode {
  position: absolute;
  top: var(--copper-space-4);
  left: var(--copper-space-4);
}

.home-page__simple {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-4);
  padding: var(--copper-space-6);
  max-width: 480px;
}

.home-page__hero {
  font-size: 28px;
  font-weight: 700;
  letter-spacing: 0.02em;
}

.home-page__version {
  min-height: var(--copper-control-h);
  display: flex;
  align-items: center;
  justify-content: center;
}

.home-page__version-name {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-lg);
}

.home-page__version-select :deep(select) {
  background: transparent;
  border-color: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-lg);
  font-weight: 500;
  height: calc(var(--copper-control-h) + 4px);
}

.home-page__actions {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  margin-top: var(--copper-space-2);
}

.home-page__actions .co-btn--primary {
  min-width: 160px;
  height: 44px;
  font-size: var(--copper-font-size-lg);
  border-radius: var(--copper-radius-lg);
  box-shadow: 0 4px 18px color-mix(in srgb, var(--copper-accent) 35%, transparent);
}

.home-page__empty-icon {
  color: var(--copper-text-disabled);
}

.home-page__empty {
  color: var(--copper-text-secondary);
  text-align: center;
}

.home-page__hint {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
  text-align: center;
}

.home-page__default-placeholder {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-3);
  color: var(--copper-text-secondary);
}
</style>
