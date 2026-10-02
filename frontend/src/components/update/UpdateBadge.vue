<script setup lang="ts">
// 标题栏更新入口：仅在「有新版本 / 下载中 / 已就绪 / 检查失败」时出现。
//
// 为什么放标题栏而不是只放在设置页：更新是**跨页面、跨时段**的事——
// 用户可能下载完就切去下载游戏了，没有一个常驻入口就永远回不去。
//
// 视觉上区分三种值得注意的状态：
// - `available`：主色实心 + 呼吸光晕（可下载，等用户决定）；
// - `downloading`：主色描边 + 环形进度（正在进行）；
// - `downloaded`：成功色实心 + 脉冲（已完成，只差一次点击）。
import { computed } from "vue";
import { ArrowUpCircle, Download, LoaderCircle, RefreshCw } from "@lucide/vue";

import { useUpdate } from "../../composables/useUpdate";
import { useI18n } from "../../i18n";

const { t } = useI18n();
const { phase, visible, downloading, ready, hasUpdate, progress, openPanel } = useUpdate();

/** 环形进度比例（0~1）；无进度时退化为不确定态。 */
const ringRatio = computed(() => {
  if (!downloading.value) return null;
  return progress.value?.ratio ?? null;
});

/** 环形 stroke-dasharray 偏移。 */
const dashOffset = computed(() => {
  const ratio = ringRatio.value;
  const circumference = 2 * Math.PI * 9;
  if (ratio == null) return circumference;
  return circumference * (1 - Math.min(1, Math.max(0, ratio)));
});

const icon = computed(() => {
  if (downloading.value) return Download;
  if (ready.value) return ArrowUpCircle;
  if (phase.value === "failed") return RefreshCw;
  return ArrowUpCircle;
});

const label = computed(() => {
  if (ready.value) return t("update.badge.ready");
  if (downloading.value) return t("update.badge.downloading");
  if (phase.value === "failed") return t("update.badge.failed");
  return t("update.badge.available");
});

const modifier = computed(() => {
  if (ready.value) return "ready";
  if (downloading.value) return "downloading";
  if (phase.value === "failed") return "failed";
  if (hasUpdate.value) return "available";
  return "";
});
</script>

<template>
  <button
    v-if="visible"
    class="update-badge"
    :class="[`update-badge--${modifier}`, { 'update-badge--pulse': hasUpdate }]"
    type="button"
    :title="label"
    :aria-label="label"
    @click="openPanel"
  >
    <svg class="update-badge__ring" viewBox="0 0 24 24" aria-hidden="true">
      <circle class="update-badge__ring-track" cx="12" cy="12" r="9" />
      <circle
        class="update-badge__ring-bar"
        cx="12"
        cy="12"
        r="9"
        :stroke-dashoffset="dashOffset"
      />
    </svg>
    <LoaderCircle
      v-if="downloading && ringRatio == null"
      class="update-badge__spin"
      :size="15"
    />
    <component :is="icon" v-else class="update-badge__icon" :size="15" />
  </button>
</template>

<style scoped>
.update-badge {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border: 1px solid transparent;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing);
}

.update-badge:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

/* 有新版本：主色实心 + 呼吸光晕，从余光里也能注意到。 */
.update-badge--available {
  background: color-mix(in srgb, var(--copper-accent) 18%, transparent);
  border-color: color-mix(in srgb, var(--copper-accent) 45%, transparent);
  color: var(--copper-accent);
  animation: update-badge-breathe 2.4s var(--copper-easing) infinite;
}

/* 下载中：描边 + 环形进度，不做呼吸（进度条已经在动，再加呼吸只会吵）。 */
.update-badge--downloading {
  border-color: color-mix(in srgb, var(--copper-accent) 45%, transparent);
  color: var(--copper-accent);
}

/* 已就绪：成功色实心 + 更快的脉冲，语义是「只差一次点击就能用上」。 */
.update-badge--ready {
  background: color-mix(in srgb, var(--copper-success) 18%, transparent);
  border-color: color-mix(in srgb, var(--copper-success) 50%, transparent);
  color: var(--copper-success);
  animation: update-badge-pulse 1.6s var(--copper-easing) infinite;
}

.update-badge--failed {
  color: var(--copper-warning);
}

.update-badge__icon,
.update-badge__spin {
  position: absolute;
}

.update-badge__spin {
  animation: update-badge-spin 1s linear infinite;
}

/* 环形进度：轨迹常隐，条形随字节比例增长。 */
.update-badge__ring {
  position: absolute;
  inset: 3px;
  transform: rotate(-90deg);
  pointer-events: none;
}

.update-badge__ring-track,
.update-badge__ring-bar {
  fill: none;
  stroke-width: 2;
}

.update-badge__ring-track {
  stroke: color-mix(in srgb, currentColor 18%, transparent);
}

.update-badge__ring-bar {
  stroke: currentColor;
  stroke-linecap: round;
  stroke-dasharray: 56.55;
  transition: stroke-dashoffset var(--copper-duration) var(--copper-easing);
}

/* 总量未知时环形不做定量（dashoffset 保持满格），由 spinner 表达进行中。 */
.update-badge--downloading .update-badge__ring-bar {
  stroke-dasharray: none;
}

@keyframes update-badge-breathe {
  0%,
  100% {
    box-shadow: 0 0 0 0 color-mix(in srgb, var(--copper-accent) 45%, transparent);
  }
  55% {
    box-shadow: 0 0 0 6px color-mix(in srgb, var(--copper-accent) 0%, transparent);
  }
}

@keyframes update-badge-pulse {
  0%,
  100% {
    box-shadow: 0 0 0 0 color-mix(in srgb, var(--copper-success) 50%, transparent);
  }
  60% {
    box-shadow: 0 0 0 7px color-mix(in srgb, var(--copper-success) 0%, transparent);
  }
}

@keyframes update-badge-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>