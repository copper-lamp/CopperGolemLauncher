<script setup lang="ts">
// 左导航栏：顶部软件图标，底部设置入口，其上方为下载入口。
// 单 icon 无文字，不展开。下载为全量列表页（/downloads），无悬浮窗。

import { computed } from "vue";
import { RouterLink } from "vue-router";
import { useRoute } from "vue-router";
import NavIcon from "./NavIcon.vue";
import DownloadNavIcon from "./DownloadNavIcon.vue";
const route = useRoute();
import { navigationScopeOf } from "../composables/useNavigationTrail";

import { useI18n } from "../i18n";
import { getModuleNav } from "../modules/registry";

const { t } = useI18n();
const moduleNav = computed(getModuleNav);
</script>

<template>
  <nav class="side-nav">
    <div class="side-nav__top">
      <RouterLink to="/" class="side-nav__logo" :title="t('app.name')">
        <img src="/app-256.png" width="26" height="26" alt="" aria-hidden="true" />
      </RouterLink>
      <RouterLink
        v-for="item in moduleNav"
        :key="item.id"
        :to="item.path"
        class="side-nav__item"
        :title="t(item.titleKey)"
      >
        <NavIcon
          :id="item.id"
          :fallback="item.icon"
          :size="22"
          :active="navigationScopeOf(route) === `module:${item.id}`"
        />
      </RouterLink>
    </div>
    <div class="side-nav__bottom">
      <RouterLink
        id="copper-nav-downloads"
        to="/downloads"
        class="side-nav__item"
        :title="t('nav.downloads')"
      >
        <DownloadNavIcon :size="22" :active="route.path === '/downloads'" />
      </RouterLink>
      <RouterLink
        to="/settings"
        class="side-nav__item"
        :title="t('nav.settings')"
      >
        <NavIcon id="settings" :size="22" :active="route.path === '/settings'" />
      </RouterLink>
    </div>
  </nav>
</template>

<style scoped>
.side-nav {
  position: relative;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  width: var(--copper-nav-w);
  flex-shrink: 0;
  padding: var(--copper-space-3) 0;
  background: var(--copper-surface);
  box-shadow: 2px 0 10px color-mix(in srgb, #000 4%, transparent);
  overflow-y: auto;
}

/*
 * 移动端（安卓锁定横屏）左侧安全区就是刘海 / 挖孔所在。
 *
 * 外壳在移动端不再为左侧预留 padding（见 base.css），所以导航的边框盒本来就从
 * 屏幕物理边缘开始。这里只需「加宽 + 等量内边距」：内容盒宽度仍是原来的导航
 * 宽度，图标位置不变，而**导航自身的 `--copper-surface` 背景一路铺进刘海区**——
 * 否则那一段会露出外壳的页面底色，看起来就是「标签页左边的刘海区域没有覆盖」。
 *
 * 不能用负外边距把导航往左推：`.app-shell` 带 `overflow: hidden`，越界部分会被裁掉。
 * 桌面端 inset 恒为 0，本规则等价于无操作。
 */
html[data-form-factor="mobile"] .side-nav {
  padding-left: var(--copper-safe-left);
  width: calc(var(--copper-nav-w) + var(--copper-safe-left));
}

.side-nav__top {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
}

/*
 * 透明底图标，容器只负责 hover 反馈与点击区，不加底色以免切掉图标透明边缘。
 * 品牌区与功能区分开：Logo 下方留 16px（6px 项间距 + 10px 额外间隔），
 * 让「我是谁」和「能去哪」在视觉上成为两组。
 */
.side-nav__logo {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  margin-bottom: 10px;
  border-radius: 12px;
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
  gap: 6px;
}

/*
 * 导航项与标题栏控件共用一套度量：40px 命中框 / 12px 圆角 / 24px 以内的图标。
 * 底色默认透明，状态只靠微色阶（hover 一档，选中用主题色 14% 混合），不描边。
 */
.side-nav__item {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: 12px;
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

/* 选中 = 主题色淡底 + 主题色图标；不再用纯黑，暗色主题下也成立。 */
.side-nav__item.router-link-active {
  color: var(--copper-accent);
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
}

.side-nav__item.router-link-active:hover {
  background: color-mix(in srgb, var(--copper-accent) 20%, transparent);
}

.side-nav__item :deep(svg) {
  width: 22px;
  height: 22px;
  flex: 0 0 22px;
}

.side-nav__item--pulse {
  border-radius: 12px;
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
