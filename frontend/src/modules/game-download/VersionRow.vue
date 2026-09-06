<script setup lang="ts">
// 版本行：供清单页的「最新」区块与分组列表复用。
// 展示版本号 / 类型徽标 / 状态徽标，并提供 下载 / 取消 / 重试 等操作与下载进度。

import { computed } from "vue";
import { useRouter } from "vue-router";
import { CheckCircle2, Download, LoaderCircle, RefreshCw, X } from "@lucide/vue";

import { useI18n } from "../../i18n";
import {
  useGameDownload,
} from "./useGameDownload";
import type { GameVersionView } from "./api";

const props = defineProps<{ version: GameVersionView; featured?: boolean }>();

const { t } = useI18n();
const router = useRouter();
const gd = useGameDownload();

const MB_KEY = "module.game-download";

/** 该版本是否处于下载中 / 排队中。 */
const isDownloading = computed(() => !!gd.liveTasks.value[props.version.id]);
/** 是否处于解包阶段（后端任务视图标记 extracting）。 */
const isExtracting = computed(
  () => gd.taskStates.value[props.version.id]?.state === "extracting",
);
const failed = computed(() => {
  const state = gd.taskStates.value[props.version.id];
  return state?.state === "failed" ? (state.error ?? "") : null;
});
const percent = computed(() => gd.percentOf(props.version.id));
const raw = computed(() => gd.rawOf(props.version.id));
const speed = computed(() => gd.speedOf(props.version.id));

function open() {
  void router.push(`/game-download/${encodeURIComponent(props.version.id)}`);
}

async function onDownload() {
  await gd.enqueue(props.version.id);
}

async function onCancel() {
  await gd.cancel(props.version.id);
}
</script>

<template>
  <div class="version-row" :class="{ 'version-row--featured': featured }">
    <button class="version-row__main" @click="open">
      <div class="version-row__info">
        <div class="version-row__line">
          <span class="version-row__name">{{ version.game_version }}</span>
          <span
            class="version-row__kind"
            :class="`version-row__kind--${version.kind}`"
          >
            {{ t(`${MB_KEY}.kind.${version.kind}`) }}
          </span>
          <span class="version-row__latest" v-if="version.is_latest">
            {{ t(`${MB_KEY}.latest`) }}
          </span>
        </div>
        <div class="version-row__badges">
          <span
            v-if="version.is_installed"
            class="version-row__badge version-row__badge--installed"
          >
            <CheckCircle2 :size="13" />
            {{ t(`${MB_KEY}.installed`) }}
          </span>
          <span
            v-else-if="version.is_downloaded && !isDownloading"
            class="version-row__badge"
          >
            {{ t(`${MB_KEY}.downloaded`) }}
          </span>
        </div>
      </div>
    </button>

    <div class="version-row__action">
      <!-- 解包中 -->
      <div v-if="isExtracting" class="version-row__state">
        <LoaderCircle :size="15" class="spin" />
        <span>{{ t(`${MB_KEY}.state.extracting`) }}</span>
      </div>
      <!-- 下载中 / 排队中 -->
      <div v-else-if="isDownloading" class="version-row__state">
        <div class="version-row__progress">
          <div class="version-row__progress-track">
            <div
              class="version-row__progress-bar"
              :style="{ width: `${percent * 100}%` }"
            />
          </div>
          <span class="version-row__progress-text">
            {{ Math.round(percent * 100) }}%<template v-if="raw"> · {{ raw }}</template>
          </span>
        </div>
        <span class="version-row__speed">{{ speed }}</span>
        <button class="version-row__icon-btn" :title="t(`${MB_KEY}.actions.cancel`)" @click="onCancel">
          <X :size="15" />
        </button>
      </div>
      <!-- 失败：可重试 -->
      <div v-else-if="failed !== null" class="version-row__state version-row__state--failed">
        <button
          class="version-row__btn version-row__btn--accent"
          :title="failed || undefined"
          @click="onDownload"
        >
          <RefreshCw :size="14" />
          {{ t(`${MB_KEY}.actions.retry`) }}
        </button>
      </div>
      <!-- 已安装 -->
      <span
        v-else-if="version.is_installed"
        class="version-row__btn version-row__btn--ghost"
      >
        <CheckCircle2 :size="14" />
        {{ t(`${MB_KEY}.installed`) }}
      </span>
      <!-- 可下载 -->
      <button v-else class="version-row__btn version-row__btn--accent" @click="onDownload">
        <Download :size="14" />
        {{ t(`${MB_KEY}.actions.download`) }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.version-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-2) var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  border: 1px solid transparent;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing);
}

.version-row:hover {
  background: var(--copper-surface-2);
  border-color: var(--copper-border);
}

.version-row--featured {
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
}

.version-row__main {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  border: none;
  background: transparent;
  color: var(--copper-text);
  text-align: left;
  cursor: pointer;
  padding: 0;
}

.version-row__info {
  min-width: 0;
}

.version-row__line {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  flex-wrap: wrap;
}

.version-row__name {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
}

.version-row__kind {
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  font-size: var(--copper-font-size-xs);
}

.version-row__kind--release {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
  color: var(--copper-accent);
}

.version-row__kind--preview {
  background: color-mix(in srgb, var(--copper-danger) 14%, transparent);
  color: var(--copper-danger);
}

.version-row__latest {
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-3);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.version-row__badges {
  display: flex;
  gap: var(--copper-space-1);
  margin-top: 2px;
}

.version-row__badge {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.version-row__badge--installed {
  color: var(--copper-success);
}

.version-row__action {
  flex-shrink: 0;
  min-width: 200px;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--copper-space-2);
}

.version-row__state {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.version-row__state--failed {
  color: var(--copper-danger);
}

.version-row__progress {
  display: flex;
  flex-direction: column;
  gap: 2px;
  width: 160px;
}

.version-row__progress-track {
  height: 5px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-3);
  overflow: hidden;
}

.version-row__progress-bar {
  height: 100%;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  transition: width var(--copper-duration-fast) var(--copper-easing);
}

.version-row__progress-text {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-secondary);
}

.version-row__speed {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-secondary);
  white-space: nowrap;
}

.version-row__icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.version-row__icon-btn:hover {
  background: var(--copper-danger);
  color: var(--copper-on-accent, #fff);
}

.version-row__btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  border: 1px solid transparent;
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  white-space: nowrap;
  transition: opacity var(--copper-duration-fast) var(--copper-easing);
}

.version-row__btn--accent {
  background: var(--copper-accent);
  color: var(--copper-on-accent, #fff);
}

.version-row__btn--accent:hover {
  opacity: 0.9;
}

.version-row__btn--ghost {
  border-color: var(--copper-border);
  background: var(--copper-surface);
  color: var(--copper-text-secondary);
  cursor: default;
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