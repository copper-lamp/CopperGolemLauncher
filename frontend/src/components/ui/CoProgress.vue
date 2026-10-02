<script setup lang="ts">
// 进度条：确定态（已知百分比）与不确定态（总量未知 / 检查中）。
//
// 之所以需要不确定态：更新包的 `Content-Length` 可能缺失（GitHub 资产走重定向时
// 会给 `Content-Length`，但代理与镜像不一定）。此时若强行画 0% 的条，
// 用户看到的就是「卡住了」——一个必须区分开的两种状态。

import { computed } from "vue";

const props = withDefaults(
  defineProps<{
    /** 0~1；`null` 表示不确定态。 */
    value: number | null;
    /** 轨道高度（px）。 */
    size?: "sm" | "md";
    /** 高亮色语义：主色 / 成功 / 危险。 */
    tone?: "accent" | "success" | "danger";
  }>(),
  { size: "md", tone: "accent" },
);

/** 百分比文本（0~100，确定态才有）。 */
const percent = computed(() =>
  props.value == null ? null : Math.round(Math.min(1, Math.max(0, props.value)) * 100),
);
</script>

<template>
  <div
    class="co-progress"
    :class="[
      `co-progress--${size}`,
      `co-progress--${tone}`,
      { 'co-progress--indeterminate': value === null },
    ]"
    role="progressbar"
    :aria-valuemin="0"
    :aria-valuemax="100"
    :aria-valuenow="percent ?? undefined"
    :aria-valuetext="percent == null ? undefined : `${percent}%`"
  >
    <div class="co-progress__fill" :style="percent == null ? undefined : { width: `${percent}%` }" />
  </div>
</template>

<style scoped>
.co-progress {
  position: relative;
  width: 100%;
  overflow: hidden;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-3);
}

.co-progress--sm {
  height: 4px;
}

.co-progress--md {
  height: 6px;
}

.co-progress__fill {
  height: 100%;
  border-radius: inherit;
  background: var(--copper-accent);
  /* 宽度过渡：引擎 200ms 广播一次进度，插值让推进看起来连续而不是跳格 */
  transition: width var(--copper-duration) var(--copper-easing);
}

.co-progress--success .co-progress__fill {
  background: var(--copper-success);
}

.co-progress--danger .co-progress__fill {
  background: var(--copper-danger);
}

/* 不确定态：条身满格但整体左右滑动，语义是「在动但不知道还剩多少」。 */
.co-progress--indeterminate .co-progress__fill {
  width: 35%;
  animation: co-progress-slide 1.15s var(--copper-easing) infinite;
}

@keyframes co-progress-slide {
  from {
    transform: translateX(-100%);
  }
  to {
    transform: translateX(340%);
  }
}
</style>