<script setup lang="ts">
// 徽标：全站唯一的徽标实现（游戏下载、内容下载、模块管理共用）。
//
// 视觉方案已定稿，不接受第二套：
// - **低纯度实色底**：色相保留、饱和度压到中低。一屏十几枚徽标同时出现时，高纯度
//   红/绿/蓝密集排列会互相抢眼，读起来像一堆告警；压低纯度后它们退回背景层。
// - **纯白文字与图标**：语义全部由底色承担，文字不再与底色抢对比度——小字号下
//   这才是可读性的第一瓶颈（语义色文字压在同色系淡底上只有 2:1 上下）。
// - **扁平**：无描边、无内高光、无内阴影。徽标是配角，任何厚度感都会让它看起来
//   像一个可以点的按钮；真要能点的东西另有按钮组件。
//
// 因此徽标既不随深浅主题变体（实心色块不是表面色，跟着主题变浅变深只会在深色
// 主题下变成一块发灰的补丁），也不需要两套色值。
//
// 配色一律走 `--copper-badge-<tone>` 令牌，禁止颜色字面量。

import type { Component } from "vue";
import { computed } from "vue";

/**
 * 徽标语义色。分组对应不同维度：
 * - 发布类型（release / beta / alpha / preview）；
 * - 内容类型与来源（内容下载模块，三个维度必须错开色相，否则同一条内容上的三枚
 *   徽标会撞成同一个颜色，区分就失去意义）；
 * - 通用状态（success / warning / danger / accent / neutral）。
 */
export type BadgeTone =
  | "release"
  | "beta"
  | "alpha"
  | "preview"
  | "ll-mod"
  | "downloaded"
  | "behavior-pack"
  | "texture-pack"
  | "shader"
  | "source-curseforge"
  | "source-lip"
  | "source-lla"
  | "success"
  | "warning"
  | "danger"
  | "accent"
  | "neutral";

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
  border-radius: var(--copper-radius-full);
  /* 扁平：无描边、无阴影，一行都不给。 */
  color: var(--copper-on-solid);
  background: var(--copper-badge-neutral);
  font-size: var(--copper-font-size-xs);
  font-weight: 600;
  /* 微字距：小字号下默认字距会让中文徽标挤成一团。 */
  letter-spacing: 0.3px;
  line-height: 1.5;
  white-space: nowrap;
}

.co-badge--sm {
  padding: 1px 7px;
  font-size: 10px;
  gap: 3px;
}

.co-badge__icon {
  flex-shrink: 0;
  color: currentColor;
}

/* 发布类型 */
.co-badge--release {
  background: var(--copper-badge-release);
}

.co-badge--beta {
  background: var(--copper-badge-beta);
}

.co-badge--alpha,
.co-badge--preview {
  background: var(--copper-badge-alpha);
}

/* 内容类型与来源 */
.co-badge--ll-mod {
  background: var(--copper-badge-ll-mod);
}

.co-badge--behavior-pack {
  background: var(--copper-badge-behavior-pack);
}

.co-badge--texture-pack {
  background: var(--copper-badge-texture-pack);
}

.co-badge--shader {
  background: var(--copper-badge-shader);
}

.co-badge--source-curseforge {
  background: var(--copper-badge-source-curseforge);
}

.co-badge--source-lip {
  background: var(--copper-badge-source-lip);
}

.co-badge--source-lla {
  background: var(--copper-badge-source-lla);
}

/* 通用状态 */
.co-badge--downloaded {
  background: var(--copper-badge-downloaded);
}

.co-badge--success {
  background: var(--copper-badge-success);
}

.co-badge--warning {
  background: var(--copper-badge-warning);
}

.co-badge--danger {
  background: var(--copper-badge-danger);
}

.co-badge--accent {
  background: var(--copper-badge-accent);
}
</style>