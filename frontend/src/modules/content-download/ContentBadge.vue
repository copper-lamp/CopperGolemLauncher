<script setup lang="ts">
// 内容徽标：**CoBadge 的薄适配层**，不是第二套实现。
//
// 它只负责一件事：把「文案可能为空」这件事收进来。内容下载的徽标文案来自 i18n
// 键推导（`typeBadgeLabel` 缺键时返回空串表示「不渲染」），而 CoBadge 是纯展示组件、
// 不该知道 i18n 的缺键语义。视觉（低纯度实色底 + 纯白文字 + 扁平无描边）全部在
// CoBadge 里，这里一行样式都没有——两处各写一份徽标 CSS 迟早漂移。

import CoBadge from "../../components/ui/CoBadge.vue";
import type { BadgeTone } from "./badges";

defineProps<{
  /** 色调（决定取哪一组令牌）。 */
  tone: BadgeTone;
  /** 文案（空串则不渲染）。 */
  label: string;
}>();
</script>

<template>
  <CoBadge v-if="label" :tone="tone">{{ label }}</CoBadge>
</template>