<script setup lang="ts">
// 徽标：全站唯一的徽标实现（游戏下载、内容下载、模块管理共用）。
//
// 视觉方案已定稿，不接受第二套：
// - **高饱和深底 + 同色相淡亮字**：鲜艳度全部由底色承担，文字保留色相而不是死白。
//   纯白字方案会把底色明度锁死在对比度下限上，12 个色调只能落进一片暗绿/暗蓝里
//   （详见 styles/tokens.css 徽标段落的选值推导）。
// - **纯文字，无图标**：图标是版式噪声。同一枚徽标在游戏下载带图标、在内容下载不带，
//   两边就对不上了；徽标要做的是让「类型 / 来源 / 发布渠道」三个维度可区分，图标帮不上。
// - **胶囊形、扁平**：无描边、无内高光、无内阴影。徽标是配角，任何厚度感都会让它看起来
//   像一个可以点的按钮；真要能点的东西另有按钮组件。
//
// 因此徽标既不随深浅主题变体（实心色块不是表面色，跟着主题变浅变深只会在深色
// 主题下变成一块发灰的补丁），也不需要两套色值。
//
// 配色一律走 `--copper-badge-<tone>`（底）与 `--copper-badge-<tone>-fg`（字）令牌，
// 禁止颜色字面量。

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
    /**
     * 文字内容。传空串 / 缺省 **不渲染** 任何东西。
     *
     * 内容下载模块的映射函数遇到无对应文案的类型时返回空串表示「这一维度没有徽标」，
     * 这个约定直接由组件兜住，不再需要调用方自己 `v-if`。
     */
    label?: string;
    /** 无障碍读法；文字本身可读时可省略。 */
    title?: string;
  }>(),
  { size: "md", label: undefined, title: undefined },
);

const toneClass = computed(() => `co-badge--${props.tone}`);

const fgClass = computed(() => `co-badge--${props.tone}-fg`);

const visible = computed(() => (props.label ?? "").trim().length > 0);
</script>

<template>
  <span
    v-if="visible"
    class="co-badge"
    :class="[toneClass, fgClass, `co-badge--${size}`]"
    :title="title"
  >
    {{ label }}
  </span>
</template>

<style scoped>
.co-badge {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  padding: 2px 9px;
  border-radius: var(--copper-radius-full);
  /* 扁平：无描边、无阴影，一行都不给。 */
  color: var(--copper-badge-neutral-fg);
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
}

/* 发布类型 */
.co-badge--release {
  background: var(--copper-badge-release);
}
.co-badge--release-fg {
  color: var(--copper-badge-release-fg);
}

.co-badge--beta {
  background: var(--copper-badge-beta);
}
.co-badge--beta-fg {
  color: var(--copper-badge-beta-fg);
}

.co-badge--alpha,
.co-badge--preview {
  background: var(--copper-badge-alpha);
}
.co-badge--alpha-fg,
.co-badge--preview-fg {
  color: var(--copper-badge-alpha-fg);
}

/* 内容类型与来源 */
.co-badge--ll-mod {
  background: var(--copper-badge-ll-mod);
}
.co-badge--ll-mod-fg {
  color: var(--copper-badge-ll-mod-fg);
}

.co-badge--behavior-pack {
  background: var(--copper-badge-behavior-pack);
}
.co-badge--behavior-pack-fg {
  color: var(--copper-badge-behavior-pack-fg);
}

.co-badge--texture-pack {
  background: var(--copper-badge-texture-pack);
}
.co-badge--texture-pack-fg {
  color: var(--copper-badge-texture-pack-fg);
}

.co-badge--shader {
  background: var(--copper-badge-shader);
}
.co-badge--shader-fg {
  color: var(--copper-badge-shader-fg);
}

.co-badge--source-curseforge {
  background: var(--copper-badge-source-curseforge);
}
.co-badge--source-curseforge-fg {
  color: var(--copper-badge-source-curseforge-fg);
}

.co-badge--source-lip {
  background: var(--copper-badge-source-lip);
}
.co-badge--source-lip-fg {
  color: var(--copper-badge-source-lip-fg);
}

.co-badge--source-lla {
  background: var(--copper-badge-source-lla);
}
.co-badge--source-lla-fg {
  color: var(--copper-badge-source-lla-fg);
}

/* 通用状态 */
.co-badge--downloaded {
  background: var(--copper-badge-downloaded);
}
.co-badge--downloaded-fg {
  color: var(--copper-badge-downloaded-fg);
}

.co-badge--success {
  background: var(--copper-badge-success);
}
.co-badge--success-fg {
  color: var(--copper-badge-success-fg);
}

.co-badge--warning {
  background: var(--copper-badge-warning);
}
.co-badge--warning-fg {
  color: var(--copper-badge-warning-fg);
}

.co-badge--danger {
  background: var(--copper-badge-danger);
}
.co-badge--danger-fg {
  color: var(--copper-badge-danger-fg);
}

.co-badge--accent {
  background: var(--copper-badge-accent);
}
.co-badge--accent-fg {
  color: var(--copper-badge-accent-fg);
}
</style>
