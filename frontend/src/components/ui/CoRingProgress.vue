<script setup lang="ts">
// 环形进度（不确定态）：一段固定长度的弧绕圆心旋转。
//
// 只用于「时长不可预测」的等待（如游戏启动确认）。不显示百分比 —— 启动耗时
// 由系统与游戏自身决定，编一个数字出来只会误导用户。

import { computed } from "vue";

const props = withDefaults(
  defineProps<{
    /** 直径（像素）。 */
    size?: number;
    /** 线宽（像素）。 */
    stroke?: number;
  }>(),
  {
    size: 16,
    stroke: 2,
  },
);

const radius = computed(() => (props.size - props.stroke) / 2);
const circumference = computed(() => 2 * Math.PI * radius.value);
</script>

<template>
  <svg
    class="co-ring"
    :width="size"
    :height="size"
    viewBox="0 0 16 16"
    role="presentation"
    aria-hidden="true"
    focusable="false"
  >
    <circle
      class="co-ring__track"
      cx="8"
      cy="8"
      :r="radius"
      fill="none"
      :stroke-width="stroke"
    />
    <circle
      class="co-ring__arc"
      cx="8"
      cy="8"
      :r="radius"
      fill="none"
      stroke="currentColor"
      stroke-linecap="round"
      :stroke-width="stroke"
      :stroke-dasharray="`${circumference * 0.25} ${circumference}`"
    />
  </svg>
</template>

<style scoped>
.co-ring {
  display: block;
  flex-shrink: 0;
}

.co-ring__track {
  stroke: color-mix(in srgb, currentColor 25%, transparent);
}

.co-ring__arc {
  transform-origin: 50% 50%;
  animation: co-ring-spin 0.9s linear infinite;
}

@keyframes co-ring-spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .co-ring__arc {
    animation-duration: 2.4s;
  }
}
</style>
