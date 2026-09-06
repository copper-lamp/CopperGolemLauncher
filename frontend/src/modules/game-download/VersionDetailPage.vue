<script setup lang="ts">
// 版本详情页：展示版本元数据与实时任务状态（下载进度 / 解包 / 结果），
// 提供投递 / 取消 / 重试操作。数据来自模块单例（`useGameDownload`）。

import { computed, onMounted, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import {
  ArrowLeft,
  CheckCircle2,
  Download,
  FileKey,
  Gamepad2,
  LoaderCircle,
  RefreshCw,
  ShieldAlert,
  X,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import { initGameDownload, useGameDownload } from "./useGameDownload";
import { formatBytes, type GameVersionView } from "./api";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const gd = useGameDownload();

const MB_KEY = "module.game-download";

const id = computed(() => String(route.params.id ?? ""));

/** 从清单视图定位当前版本（最新或分组内）。 */
const version = computed<GameVersionView | null>(() => {
  const m = gd.manifest.value;
  if (!m) return null;
  const inLatest = [m.latest_release, m.latest_preview].find((v) => v?.id === id.value);
  if (inLatest) return inLatest;
  for (const group of m.groups) {
    const v = group.items.find((i) => i.id === id.value);
    if (v) return v;
  }
  return null;
});

const state = computed(() => gd.taskStates.value[id.value]);
const live = computed(() => gd.liveTasks.value[id.value]);

const isDownloading = computed(() => !!live.value);
const isExtracting = computed(() => state.value?.state === "extracting");
const failed = computed(() =>
  state.value?.state === "failed" ? (state.value.error ?? "") : null,
);

const percent = computed(() => gd.percentOf(id.value));
const raw = computed(() => gd.rawOf(id.value));
const speed = computed(() => gd.speedOf(id.value));

async function onDownload() {
  await gd.enqueue(id.value);
}

async function onCancel() {
  await gd.cancel(id.value);
}

function goBack() {
  void router.push("/game-download");
}

onMounted(async () => {
  await initGameDownload();
  await gd.loadManifest(false);
  await gd.refreshState(id.value);
});

watch(id, () => {
  void gd.refreshState(id.value);
});
</script>

<template>
  <div class="gd-detail">
    <button class="gd-detail__back" @click="goBack">
      <ArrowLeft :size="15" />
      {{ t(`${MB_KEY}.actions.back`) }}
    </button>

    <!-- 未找到版本 -->
    <div v-if="!version" class="gd-detail__state">
      <p class="gd-detail__state-text">{{ t(`${MB_KEY}.error.no_url`) }}</p>
      <button class="gd-detail__primary" @click="goBack">
        {{ t(`${MB_KEY}.actions.back`) }}
      </button>
    </div>

    <template v-else>
      <!-- 头部：版本信息 -->
      <div class="gd-detail__hero">
        <div class="gd-detail__icon">
          <Gamepad2 :size="30" />
        </div>
        <div class="gd-detail__hero-info">
          <div class="gd-detail__hero-line">
            <span class="gd-detail__name">{{ version.game_version }}</span>
            <span
              class="gd-detail__kind"
              :class="`gd-detail__kind--${version.kind}`"
            >
              {{ t(`${MB_KEY}.kind.${version.kind}`) }}
            </span>
          </div>
          <span
            v-if="version.is_installed"
            class="gd-detail__badge gd-detail__badge--installed"
          >
            <CheckCircle2 :size="14" />
            {{ t(`${MB_KEY}.installed`) }}
          </span>
        </div>
      </div>

      <!-- 元数据 -->
      <div class="gd-detail__meta">
        <div class="gd-detail__meta-item">
          <span class="gd-detail__meta-label">
            <FileKey :size="13" /> {{ t(`${MB_KEY}.meta.md5`) }}
          </span>
          <code class="gd-detail__meta-value">{{ version.md5 || "—" }}</code>
        </div>
        <div
          class="gd-detail__meta-item"
          v-if="state?.download?.total_bytes"
        >
          <span class="gd-detail__meta-label">{{ t(`${MB_KEY}.meta.size`) }}</span>
          <span class="gd-detail__meta-value">
            {{ formatBytes(state.download.total_bytes) }}
          </span>
        </div>
      </div>

      <!-- 状态与操作 -->
      <div class="gd-detail__status">
        <!-- 解包中 -->
        <div v-if="isExtracting" class="gd-detail__state-block">
          <LoaderCircle :size="20" class="spin" />
          <span>{{ t(`${MB_KEY}.state.extracting`) }}</span>
        </div>

        <!-- 下载进度 -->
        <div v-else-if="isDownloading" class="gd-detail__state-block">
          <div class="gd-detail__progress">
            <div class="gd-detail__progress-track">
              <div
                class="gd-detail__progress-bar"
                :style="{ width: `${percent * 100}%` }"
              />
            </div>
            <div class="gd-detail__progress-caption">
              <span>
                {{ Math.round(percent * 100) }}%<template v-if="raw"> · {{ raw }}</template>
              </span>
              <span>{{ speed }}</span>
            </div>
          </div>
          <button
            class="gd-detail__ghost gd-detail__ghost--danger"
            :title="t(`${MB_KEY}.actions.cancel`)"
            @click="onCancel"
          >
            <X :size="15" />
            {{ t(`${MB_KEY}.actions.cancel`) }}
          </button>
        </div>

        <!-- 失败 -->
        <div v-else-if="failed" class="gd-detail__state-block gd-detail__state-block--error">
          <ShieldAlert :size="20" />
          <span class="gd-detail__error-text">{{ failed }}</span>
          <button class="gd-detail__primary" @click="onDownload">
            <RefreshCw :size="14" />
            {{ t(`${MB_KEY}.actions.retry`) }}
          </button>
        </div>

        <!-- 已安装 -->
        <div v-else-if="version.is_installed" class="gd-detail__state-block">
          <CheckCircle2 :size="20" class="ok" />
          <span>{{ t(`${MB_KEY}.installed`) }}</span>
          <button class="gd-detail__primary" @click="onDownload">
            <Download :size="14" />
            {{ t(`${MB_KEY}.actions.reinstall`) }}
          </button>
        </div>

        <!-- 可下载 -->
        <div v-else class="gd-detail__state-block">
          <button class="gd-detail__primary" @click="onDownload">
            <Download :size="16" />
            {{ t(`${MB_KEY}.actions.download`) }}
          </button>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.gd-detail {
  height: 100%;
  padding: var(--copper-space-5);
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-4);
  max-width: 720px;
  margin: 0 auto;
}

.gd-detail__back {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  align-self: flex-start;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition: color var(--copper-duration-fast) var(--copper-easing);
}

.gd-detail__back:hover {
  color: var(--copper-text);
}

.gd-detail__state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-3);
}

.gd-detail__state-text {
  color: var(--copper-text-secondary);
}

.gd-detail__hero {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-4);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
}

.gd-detail__icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 56px;
  height: 56px;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-accent);
  flex-shrink: 0;
}

.gd-detail__hero-info {
  min-width: 0;
}

.gd-detail__hero-line {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  flex-wrap: wrap;
}

.gd-detail__name {
  font-size: var(--copper-font-size-lg);
  font-weight: 700;
}

.gd-detail__kind {
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  font-size: var(--copper-font-size-xs);
}

.gd-detail__kind--release {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
  color: var(--copper-accent);
}

.gd-detail__kind--preview {
  background: color-mix(in srgb, var(--copper-danger) 14%, transparent);
  color: var(--copper-danger);
}

.gd-detail__badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-top: var(--copper-space-1);
  font-size: var(--copper-font-size-xs);
}

.gd-detail__badge--installed {
  color: var(--copper-success);
}

.gd-detail__meta {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
}

.gd-detail__meta-item {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  font-size: var(--copper-font-size-sm);
}

.gd-detail__meta-label {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  color: var(--copper-text-secondary);
  min-width: 90px;
}

.gd-detail__meta-value {
  color: var(--copper-text);
  word-break: break-all;
}

.gd-detail__status {
  padding: var(--copper-space-4);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
}

.gd-detail__state-block {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  flex-wrap: wrap;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.gd-detail__state-block--error {
  color: var(--copper-danger);
}

.gd-detail__error-text {
  flex: 1;
  min-width: 180px;
  word-break: break-word;
}

.gd-detail__progress {
  flex: 1;
  min-width: 220px;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.gd-detail__progress-track {
  height: 8px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-3);
  overflow: hidden;
}

.gd-detail__progress-bar {
  height: 100%;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  transition: width var(--copper-duration-fast) var(--copper-easing);
}

.gd-detail__progress-caption {
  display: flex;
  justify-content: space-between;
  font-size: var(--copper-font-size-xs);
}

.gd-detail__primary,
.gd-detail__ghost {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-4);
  border-radius: var(--copper-radius-md);
  border: 1px solid transparent;
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  white-space: nowrap;
  transition: opacity var(--copper-duration-fast) var(--copper-easing);
}

.gd-detail__primary {
  background: var(--copper-accent);
  color: var(--copper-on-accent, #fff);
}

.gd-detail__primary:hover {
  opacity: 0.9;
}

.gd-detail__ghost {
  background: var(--copper-surface);
  border-color: var(--copper-border);
  color: var(--copper-text);
}

.gd-detail__ghost:hover {
  background: var(--copper-surface-2);
}

.gd-detail__ghost--danger {
  color: var(--copper-danger);
  border-color: color-mix(in srgb, var(--copper-danger) 35%, var(--copper-border));
}

.ok {
  color: var(--copper-success);
}

.spin {
  animation: spin 1.2s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>