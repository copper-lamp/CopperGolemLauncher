<script setup lang="ts">
// 左导航栏：顶部软件图标，底部设置入口，其上方为下载入口。
// 单 icon 无文字，不展开。下载为全量列表页（/downloads），无悬浮窗。

import { RouterLink } from "vue-router";
import { Settings, ArrowDownToLine } from "@lucide/vue";

import { useI18n } from "../i18n";
import { getModuleNav } from "../modules/registry";

const { t } = useI18n();
const moduleNav = getModuleNav();
</script>

<template>
  <nav class="side-nav">
    <div class="side-nav__top">
      <RouterLink to="/" class="side-nav__logo" :title="t('app.name')">
        <img src="/app-64.png" width="26" height="26" alt="" aria-hidden="true" />
      </RouterLink>
      <RouterLink
        v-for="item in moduleNav"
        :key="item.id"
        :to="item.path"
        class="side-nav__item"
        :title="t(item.titleKey)"
      >
        <component :is="item.icon" :size="20" />
      </RouterLink>
    </div>
    <div class="side-nav__bottom">
      <RouterLink
        id="copper-nav-downloads"
        to="/downloads"
        class="side-nav__item"
        :title="t('nav.downloads')"
      >
        <ArrowDownToLine :size="20" />
      </RouterLink>
      <RouterLink
        to="/settings"
        class="side-nav__item"
        :title="t('nav.settings')"
      >
        <Settings :size="20" />
      </RouterLink>
    </div>
  </nav>
</template>

<style scoped>
.side-nav {
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  width: var(--copper-nav-w);
  flex-shrink: 0;
  padding: var(--copper-space-2) 0;
  background: var(--copper-surface);
  border-right: 1px solid var(--copper-border);
}

.side-nav__top {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-3);
}

/* 透明底图标，容器只负责 hover 反馈与点击区，不加底色以免切掉图标透明边缘 */
.side-nav__logo {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: var(--copper-radius-md);
  text-decoration: none;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.side-nav__logo img {
  width: 26px;
  height: 26px;
}

.side-nav__logo:hover {
  background: var(--copper-hover);
  transform: scale(1.06);
}

.side-nav__logo:active {
  transform: scale(0.96);
}

.side-nav__bottom {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-2);
}

.side-nav__item {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: var(--copper-radius-md);
  color: var(--copper-text-secondary);
  text-decoration: none;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.side-nav__item:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.side-nav__item.router-link-active {
  background: color-mix(in srgb, var(--copper-accent) 16%, transparent);
  color: var(--copper-accent);
}

/* 下载落点水波纹：圆点飞抵下载入口后触发，向外扩散一圈即消散。 */
.side-nav__item--pulse {
  position: relative;
  color: var(--copper-accent);
}

.side-nav__item--pulse::after {
  content: "";
  position: absolute;
  inset: 0;
  border-radius: inherit;
  border: 2px solid var(--copper-accent);
  pointer-events: none;
  animation: side-nav-ripple 700ms var(--copper-easing) forwards;
}

@keyframes side-nav-ripple {
  0% {
    transform: scale(0.9);
    opacity: 0.85;
  }
  100% {
    transform: scale(2.1);
    opacity: 0;
  }
}
</style>
