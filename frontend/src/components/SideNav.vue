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
        <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
          <path
            d="M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Zm3 8h8v2H8v-2Zm0 4h5v2H8v-2Zm0-8h8v2H8V7Z"
            fill="currentColor"
          />
        </svg>
      </RouterLink>
    </div>
    <div class="side-nav__bottom">
      <RouterLink
        v-for="item in moduleNav"
        :key="item.id"
        :to="item.path"
        class="side-nav__item"
        :title="t(item.titleKey)"
      >
        <component :is="item.icon" :size="20" />
      </RouterLink>
      <RouterLink
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

.side-nav__logo {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 38px;
  height: 38px;
  border-radius: var(--copper-radius-md);
  color: var(--copper-accent);
  background: color-mix(in srgb, var(--copper-accent) 12%, transparent);
  text-decoration: none;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.side-nav__logo:hover {
  background: color-mix(in srgb, var(--copper-accent) 20%, transparent);
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
</style>
