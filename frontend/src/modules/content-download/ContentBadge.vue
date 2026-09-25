<script setup lang="ts">
// 内容徽标：按「类型 / 来源 / 发布渠道」三个维度着色。
//
// 颜色全部取自主题令牌 `--copper-badge-<tone>`（深色 / 浅色各一套，见 styles/tokens.css），
// 组件内不写字面量颜色。`label` 为空时不渲染，由调用方决定是否展示。

import { computed } from "vue";

import type { BadgeTone } from "./badges";

const props = defineProps<{
  /** 色调（决定取哪一组令牌）。 */
  tone: BadgeTone;
  /** 文案（空串则不渲染）。 */
  label: string;
}>();

const toneVar = computed(() => `--copper-badge-${props.tone}`);
</script>

<template>
  <span
    v-if="label"
    class="cd-badge"
    :style="{
      '--badge-color': `var(${toneVar})`,
      '--badge-bg': `var(${toneVar}-bg)`,
      '--badge-border': `var(${toneVar}-border)`,
    }"
  >
    {{ label }}
  </span>
</template>

<style scoped>
.cd-badge {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  padding: 2px 9px;
  border: 1px solid var(--badge-border);
  border-radius: var(--copper-radius-full);
  background: var(--badge-bg);
  color: var(--badge-color);
  font-size: var(--copper-font-size-sm);
  font-weight: 500;
  line-height: 1.45;
  white-space: nowrap;
}
</style>
