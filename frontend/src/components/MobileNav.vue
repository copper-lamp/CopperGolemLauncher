<script setup lang="ts">
// 底部标签栏（移动端）：替代桌面左导航。
//
// 与桌面左导航的差异及原因：
// - 移动端没有 hover，单 icon 无法通过 tooltip 补足语义，故此处**带文字标签**；
// - 触控目标高度不低于 48px，并叠加底部安全区，避免被系统手势条遮挡；
// - 导航项与桌面保持同一份来源（模块注册表 + 内核 Downloads / Settings），
//   两态布局不产生第二套导航定义。

import { RouterLink } from "vue-router";
import { Settings, ArrowDownToLine } from "@lucide/vue";

import { useI18n } from "../i18n";
import { getModuleNav } from "../modules/registry";

const { t } = useI18n();
const moduleNav = getModuleNav();
</script>

<template>
  <nav class="mobile-nav">
    <RouterLink
      v-for="item in moduleNav"
      :key="item.id"
      :to="item.path"
      class="mobile-nav__item"
    >
      <component :is="item.icon" :size="22" />
      <span class="mobile-nav__label">{{ t(item.titleKey) }}</span>
    </RouterLink>
    <RouterLink to="/downloads" class="mobile-nav__item">
      <ArrowDownToLine :size="22" />
      <span class="mobile-nav__label">{{ t("nav.downloads") }}</span>
    </RouterLink>
    <RouterLink to="/settings" class="mobile-nav__item">
      <Settings :size="22" />
      <span class="mobile-nav__label">{{ t("nav.settings") }}</span>
    </RouterLink>
  </nav>
</template>

<style scoped>
.mobile-nav {
  display: flex;
  flex-shrink: 0;
  align-items: stretch;
  justify-content: space-around;
  gap: var(--copper-space-1);
  padding-bottom: var(--copper-safe-bottom);
  background: var(--copper-surface);
  border-top: 1px solid var(--copper-border);
}

.mobile-nav__item {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 2px;
  min-height: 56px;
  padding: var(--copper-space-1) var(--copper-space-1);
  color: var(--copper-text-secondary);
  text-decoration: none;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

/* 触控按下反馈：移动端无 hover，改用 :active 给出即时响应。 */
.mobile-nav__item:active {
  background: var(--copper-active);
}

.mobile-nav__item.router-link-active {
  color: var(--copper-accent);
}

.mobile-nav__label {
  max-width: 100%;
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
