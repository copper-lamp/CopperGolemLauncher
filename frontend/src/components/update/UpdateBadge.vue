<script setup lang="ts">
// 标题栏更新入口。
//
// 位置：标题栏操作按钮（`#copper-titlebar-actions`）**左边**，由 TitleBar 决定，
// 组件自身不带任何布局偏移 —— 它是一个独立控件，不参与操作区的 flex 布局，
// 因此不可能和页面注入的操作按钮挤在一起。
//
// 形态（两态）：
// - 收起：直径 32 的**圆形**，克莱因蓝底 + 白色下载图标 + 白色圆形进度环；
// - 展开：下载完成、待用户决定是否重启时，宽度变宽并露出「更新」文本，
//   进度环走满 100%。
//
// 为什么收起态没有「下载中 / 待更新」的文案：32px 的圆里塞文字会挤成不可读。
// 区分状态改用**进度环**（环的角度就是字节进度，零占用），文案只在真正需要
// 用户决策的「已完成」态出现。
import { computed } from "vue";
import { ArrowDownToLine } from "@lucide/vue";

import { useUpdate } from "../../composables/useUpdate";
import { useI18n } from "../../i18n";

const { t } = useI18n();
const { visible, downloading, ready, failed, progress, openPanel } = useUpdate();

/** 收起态只有「检查失败」需要可点但无新版本这一组合，其余靠相位决定。 */
const failedOnly = computed(() => failed.value && !downloading.value && !ready.value);

/** 进度环比例（0~1）。总量未知时为 null，走不确定态。 */
const ratio = computed(() => {
  if (ready.value) return 1;
  if (downloading.value) return progress.value?.ratio ?? null;
  return failedOnly.value ? 0 : null;
});

/** 环形周长（r = 13.5 → 2πr ≈ 84.82）。 */
const CIRCUMFERENCE = 2 * Math.PI * 13.5;

/** 进度环 dashoffset：比例越高，空白越少。 */
const dashOffset = computed(() =>
  ratio.value == null ? CIRCUMFERENCE : CIRCUMFERENCE * (1 - ratio.value),
);

/** 是否展开露出文本：仅「已下载待重启」与「检查失败」两种需要用户决策。 */
const expanded = computed(() => ready.value || failedOnly.value);

const label = computed(() => {
  if (ready.value) return t("update.badge.ready_short");
  if (failedOnly.value) return t("update.badge.failed_short");
  if (downloading.value) return t("update.badge.downloading");
  return t("update.badge.available");
});

/** 悬浮说明比 32px 圆能承载的多，放完整语义。 */
const title = computed(() => {
  if (ready.value) return t("update.ready_body");
  if (failedOnly.value) return t("update.badge.failed");
  if (downloading.value) return t("update.downloading_hint");
  return t("update.available_title");
});
</script>

<template>
  <button
    v-if="visible"
    class="update-badge"
    :class="{
      'update-badge--expanded': expanded,
      'update-badge--ready': ready,
      'update-badge--failed': failedOnly,
      'update-badge--pulse': downloading && ratio == null,
    }"
    type="button"
    :title="title"
    :aria-label="title"
    @click="openPanel"
  >
    <span class="update-badge__disc">
      <svg class="update-badge__ring" viewBox="0 0 32 32" aria-hidden="true">
        <!-- 轨道：极淡的白色，仅作为「这里是进度条」的提示，不喧宾夺主 -->
        <circle class="update-badge__ring-track" cx="16" cy="16" r="13.5" />
        <!-- 进度条：白色，随字节进度推进 -->
        <circle
          class="update-badge__ring-bar"
          cx="16"
          cy="16"
          r="13.5"
          :style="{ strokeDasharray: CIRCUMFERENCE, strokeDashoffset: dashOffset }"
        />
      </svg>
      <ArrowDownToLine class="update-badge__icon" :size="15" />
    </span>
    <span v-if="expanded" class="update-badge__label">{{ label }}</span>
  </button>
</template>

<style scoped>
.update-badge {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  height: 32px;
  padding: 0;
  border: none;
  border-radius: var(--copper-radius-full);
  /* 宽度过渡：收起 → 展开的横向拉伸必须是连续的，否则是「突然长出两个字」 */
  transition:
    padding var(--copper-duration) var(--copper-easing),
    background-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration) var(--copper-easing);
  background: var(--copper-update-blue);
  color: #ffffff;
  cursor: pointer;
}

.update-badge:hover {
  background: var(--copper-update-blue-hover);
}

.update-badge:active {
  transform: scale(0.96);
}

.update-badge:focus-visible {
  outline: 2px solid var(--copper-info);
  outline-offset: 2px;
}

/* 展开态：右侧留白 + 文案；左侧圆盘宽度不变，图标不会横向跳动。 */
.update-badge--expanded {
  padding-right: 12px;
  gap: 8px;
}

.update-badge__disc {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 32px;
  height: 32px;
  border-radius: var(--copper-radius-full);
}

.update-badge__icon {
  color: #ffffff;
  stroke-width: 2;
}

/* 进度环：白色，起点在 12 点方向（rotate -90deg 已由 CSS 处理）。 */
.update-badge__ring {
  position: absolute;
  inset: 0;
  width: 32px;
  height: 32px;
  transform: rotate(-90deg);
  pointer-events: none;
}

.update-badge__ring-track,
.update-badge__ring-bar {
  fill: none;
  stroke-width: 2.5;
  stroke-linecap: round;
}

.update-badge__ring-track {
  stroke: rgba(255, 255, 255, 0.22);
}

.update-badge__ring-bar {
  stroke: #ffffff;
  transition: stroke-dashoffset var(--copper-duration) var(--copper-easing);
}

/* 总量未知：环整体缓慢转动，表达「在动但不知道还剩多少」。 */
.update-badge--pulse .update-badge__ring-bar {
  animation: update-badge-spin 1.4s linear infinite;
  stroke-dasharray: 26 60 !important;
}

.update-badge__label {
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
  white-space: nowrap;
  color: #ffffff;
  animation: update-badge-label-in var(--copper-duration) var(--copper-easing);
}

@keyframes update-badge-spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes update-badge-label-in {
  from {
    opacity: 0;
    transform: translateX(-4px);
  }
  to {
    opacity: 1;
    transform: translateX(0);
  }
}
</style>