<script setup lang="ts">
// 提示展示条：左侧图标 + 一条随机提示，按固定间隔淡入淡出轮换。
//
// 只渲染 key 对应的 i18n 文案（不渲染后端返回的 text），这样用户切换语言时
// 正在显示的提示会立刻跟着变语言。取不到文案（语言包缺失 / 后端返回 null）时
// 整条不渲染 —— `t()` 对缺失键会回退返回键名本身，不能把 `tips.items.3` 这种
// 原始键名暴露给用户。

import { computed, onMounted, onUnmounted } from "vue";
import { Lightbulb } from "@lucide/vue";

import { useTips } from "../composables/useTips";
import { useI18n } from "../i18n";

const props = withDefaults(
  defineProps<{
    /** 轮换间隔（毫秒）。 */
    interval?: number;
    /** 紧凑模式：更小字号与间距，用于空间受限的加载态。 */
    compact?: boolean;
  }>(),
  {
    interval: 6000,
    compact: false,
  },
);

const { t } = useI18n();
const { currentTipKey, startRotation, stopRotation } = useTips();

/** 提示文案；语言包无该键时 `t()` 回退返回键名，需显式判断以隐藏整条。 */
const tipText = computed(() => {
  const key = currentTipKey.value;
  if (key === null) return "";
  const text = t(key);
  // 键名原样返回说明查不到翻译，视为无提示。
  return text === key ? "" : text.trim();
});

onMounted(() => startRotation(props.interval));
onUnmounted(() => stopRotation());
</script>

<template>
  <Transition name="tip">
    <div v-if="tipText" class="tips-rotator" :class="{ 'is-compact': compact }">
      <Lightbulb :size="compact ? 13 : 15" class="tips-rotator__icon" />
      <span class="tips-rotator__text">{{ tipText }}</span>
    </div>
  </Transition>
</template>

<style scoped>
.tips-rotator {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-2);
  max-width: 520px;
  padding: 0 var(--copper-space-4);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-md);
  text-align: center;
}

/* 紧凑模式：加载页空间紧张时的降级排版。 */
.tips-rotator.is-compact {
  gap: var(--copper-space-1);
  padding: 0 var(--copper-space-2);
  font-size: var(--copper-font-size-sm);
}

.tips-rotator__icon {
  flex-shrink: 0;
  color: var(--copper-accent);
}

.tips-rotator__text {
  overflow-wrap: anywhere;
}

/* 整条淡入淡出：提示内容整体切换，避免新旧文案叠加抖动。 */
.tip-enter-active,
.tip-leave-active {
  transition: opacity var(--copper-duration) var(--copper-easing);
}

.tip-enter-from,
.tip-leave-to {
  opacity: 0;
}
</style>
