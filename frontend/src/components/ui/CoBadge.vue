<script setup lang="ts">
// 徽标：内容徽标（游戏下载模块内的唯一实现）。
//
// 为什么独立成组件：版本清单页与版本详情页都要显示「正式版 / 测试版 / LeviLamina /
// 已下载」这几枚徽标，各写一份 CSS 的后果是两处的圆角、内边距、配色一漂移就显得
// 像两个产品。这里把**结构与质感**收敛到一处，业务方只声明语义色与图标。
//
// 质感的来源（A + D 方案）：
// - **实心底色 + 纯白文字与图标**：色雾底 + 语义色文字在小字号下对比度不够，浅色
//   主题里尤其差（绿字压在 9% 绿雾上只有 2:1 上下）。「一眼看清这是正式版 / 已下载」
//   比「底色淡雅」重要，所以底色改成实心色块，文字与图标一律纯白；
// - 底色之上叠一层上边缘内高光 + 下边缘内阴影，纯色平涂才会变成有厚度的实体；
// - 图标在前、文字在后：图标给形状记忆点，文字给出精确名字，二者缺一都会让徽标
//   在一排版本号里「糊成一个小色块」；
// - 字号小、字距略宽、字重 600：徽标是配角，不能靠放大来变醒目。
//
// 配色一律走 `--copper-badge-<tone>-solid` 令牌，禁止颜色字面量（实心色块两个主题
// 共用同一组值，深浅主题的差异只体现在 sheen / underside 的浓度上）。

import type { Component } from "vue";
import { computed } from "vue";

/** 徽标语义色，对应 `--copper-badge-<tone>-solid` 实心底色。 */
export type BadgeTone = "release" | "preview" | "loader" | "downloaded" | "neutral";

const props = withDefaults(
  defineProps<{
    tone: BadgeTone;
    /** `sm` 用于密集列表，`md` 用于详情页头部。 */
    size?: "sm" | "md";
    /** 前置图标（lucide 组件）。不传则只有文字。 */
    icon?: Component;
    /** 无障碍读法；文字本身可读时可省略。 */
    title?: string;
  }>(),
  { size: "md", icon: undefined, title: undefined },
);

const toneClass = computed(() => `co-badge--${props.tone}`);
</script>

<template>
  <span class="co-badge" :class="[toneClass, `co-badge--${size}`]" :title="title">
    <component :is="icon" v-if="icon" :size="12" class="co-badge__icon" aria-hidden="true" />
    <span class="co-badge__text"><slot /></span>
  </span>
</template>

<style scoped>
.co-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  padding: 2px 9px;
  /* 实心色块上没有描边可画：再套一圈线只会让徽标在小尺寸下发糊。 */
  border: 1px solid transparent;
  border-radius: var(--copper-radius-full);
  /* 文字与图标纯白：语义色由底色承担，文字不再与底色抢对比度。 */
  color: var(--copper-on-solid);
  font-size: var(--copper-font-size-xs);
  font-weight: 600;
  /* 微字距：小字号下默认字距会让中文徽标挤成一团。 */
  letter-spacing: 0.3px;
  line-height: 1.5;
  white-space: nowrap;
  /* 玻璃感：上边缘高光 + 下边缘内阴影，深浅主题各自的浓度由令牌给。 */
  box-shadow:
    inset 0 1px 0 var(--copper-badge-sheen),
    inset 0 -1px 0 var(--copper-badge-underside);
}

.co-badge--sm {
  padding: 1px 7px;
  font-size: 10px;
  gap: 3px;
}

.co-badge__icon {
  flex-shrink: 0;
  /* 图标不降透明度：可读性优先，形状记忆点必须和白字一样清楚。 */
  color: currentColor;
}

.co-badge--release {
  background: var(--copper-badge-release-solid);
}

.co-badge--preview {
  background: var(--copper-badge-preview-solid);
}

.co-badge--loader {
  background: var(--copper-badge-loader-solid);
}

.co-badge--downloaded {
  background: var(--copper-badge-downloaded-solid);
}

.co-badge--neutral {
  background: var(--copper-badge-neutral-solid);
}
</style>