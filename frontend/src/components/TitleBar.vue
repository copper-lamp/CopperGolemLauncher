<script setup lang="ts">
// 自定义标题栏：左侧为「返回按钮 + 当前页面标题」或「面包屑导航」，中部拖拽区，
// 右侧为页面注入的操作区 + 账户头像 + 窗口控制按钮。
// 标题 / 返回 / 面包屑均来自当前路由 meta，操作区由各页面经 Teleport 注入。
//
// 路由 meta 的 `breadcrumb` 为 `{ titleKey, path }[]`：声明后左侧渲染为导航栏样式，
// 末段为当前页（不可点击）、其余段点击跳转，且不再显示返回箭头与独立标题。

import { ref, computed, onMounted, onUnmounted } from "vue";
import { Minus, Square, Copy, X, ArrowLeft, ChevronRight } from "@lucide/vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useRoute, useRouter } from "vue-router";

import AccountMenu from "./AccountMenu.vue";
import { useI18n } from "../i18n";
import { usePlatform } from "../composables/usePlatform";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();

// 移动端没有窗口装饰，最小化 / 最大化 / 关闭按钮无意义，需隐藏（见 docs/平台适配.md 2.5）。
const { isMobile } = usePlatform();

/** 当前路由标题（i18n）与返回目标路径。 */
const title = computed(() => {
  const key = route.meta.titleKey;
  return typeof key === "string" ? t(key) : "";
});
const backPath = computed(() =>
  typeof route.meta.backPath === "string" ? route.meta.backPath : null,
);

/** 面包屑单段。 */
interface BreadcrumbItem {
  /** 段标题的 i18n 键。 */
  titleKey: string;
  /** 点击跳转路径（末段不跳转）。 */
  path: string;
}

/** 路由声明的面包屑（非数组 / 结构不合法时视为未声明）。 */
const breadcrumb = computed<BreadcrumbItem[]>(() => {
  const raw = route.meta.breadcrumb;
  if (!Array.isArray(raw)) return [];
  return raw.filter(
    (item): item is BreadcrumbItem =>
      typeof item === "object" &&
      item !== null &&
      typeof (item as BreadcrumbItem).titleKey === "string" &&
      typeof (item as BreadcrumbItem).path === "string",
  );
});

/** 面包屑跳转（保留完整路径语义，直接 push）。 */
function goCrumb(path: string) {
  void router.push(path);
}

/** 有可退历史时优先 router.back()（保留内容列表滚动位置），否则回退到 backPath。 */
function goBack() {
  const canBack = router.options.history.state.back != null;
  if (canBack) {
    void router.back();
  } else if (backPath.value) {
    void router.replace(backPath.value);
  }
}

const maximized = ref(false);
let unlistenMaximize: (() => void) | null = null;

const appWindow = getCurrentWindow();

onMounted(async () => {
  try {
    maximized.value = await appWindow.isMaximized();
    unlistenMaximize = await appWindow.onResized(() => {
      void appWindow.isMaximized().then((m) => (maximized.value = m));
    });
  } catch {
    // 非桌面环境（如浏览器调试）忽略。
  }
});

onUnmounted(() => {
  unlistenMaximize?.();
});

function minimize() {
  void appWindow.minimize();
}

function toggleMaximize() {
  void appWindow.toggleMaximize();
}

function close() {
  void appWindow.close();
}
</script>

<template>
  <header class="titlebar">
    <div class="titlebar__left">
      <nav v-if="breadcrumb.length" class="titlebar__crumbs">
        <template v-for="(crumb, index) in breadcrumb" :key="crumb.path">
          <button
            v-if="index < breadcrumb.length - 1"
            class="titlebar__crumb"
            @click="goCrumb(crumb.path)"
          >
            {{ t(crumb.titleKey) }}
          </button>
          <span v-else class="titlebar__crumb titlebar__crumb--current" aria-current="page">
            {{ t(crumb.titleKey) }}
          </span>
          <ChevronRight
            v-if="index < breadcrumb.length - 1"
            class="titlebar__crumb-sep"
            :size="13"
          />
        </template>
      </nav>
      <template v-else>
        <button
          v-if="backPath"
          class="titlebar__back"
          :title="t('titlebar.back')"
          @click="goBack"
        >
          <ArrowLeft :size="15" />
        </button>
        <h1 v-if="title" class="titlebar__title">{{ title }}</h1>
      </template>
    </div>
    <div class="titlebar__spacer" data-tauri-drag-region />
    <!-- 页面注入操作区：各模块经 <Teleport to="#copper-titlebar-actions"> 放置按钮/切换。 -->
    <div class="titlebar__actions" id="copper-titlebar-actions" />
    <div class="titlebar__right">
      <AccountMenu />
      <template v-if="!isMobile">
        <div class="titlebar__sep" />
        <div class="titlebar__controls">
          <button
            class="titlebar__btn"
            :title="t('titlebar.minimize')"
            @click="minimize"
          >
            <Minus :size="14" />
          </button>
          <button
            class="titlebar__btn"
            :title="maximized ? t('titlebar.restore') : t('titlebar.maximize')"
            @click="toggleMaximize"
          >
            <Copy v-if="maximized" :size="12" />
            <Square v-else :size="11" />
          </button>
          <button
            class="titlebar__btn titlebar__btn--close"
            :title="t('titlebar.close')"
            @click="close"
          >
            <X :size="14" />
          </button>
        </div>
      </template>
    </div>
  </header>
</template>

<style scoped>
.titlebar {
  display: flex;
  align-items: center;
  height: var(--copper-titlebar-h);
  flex-shrink: 0;
  padding-left: var(--copper-space-3);
  background: var(--copper-bg);
}

.titlebar__left {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: 100%;
  min-width: 0;
}

.titlebar__back {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.titlebar__back:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.titlebar__title {
  font-size: var(--copper-font-size-md);
  font-weight: 700;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.titlebar__crumbs {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
  min-width: 0;
}

/* 可点击段是 `button`、当前段是 `span`：两者都必须是同一种盒模型，
   否则 `height` 在 `span` 上失效、单行文本会贴着盒顶（表现为当前段文字偏上）。 */
.titlebar__crumb {
  display: inline-flex;
  align-items: center;
  height: 26px;
  padding: 0 var(--copper-space-2);
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
  line-height: 1;
  white-space: nowrap;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

button.titlebar__crumb {
  cursor: pointer;
}

button.titlebar__crumb:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.titlebar__crumb--current {
  color: var(--copper-text);
  font-weight: 700;
  cursor: default;
}

.titlebar__crumb-sep {
  flex-shrink: 0;
  color: var(--copper-text-disabled);
}

.titlebar__spacer {
  flex: 1;
  height: 100%;
}

.titlebar__actions {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: 100%;
  padding: 0 var(--copper-space-2);
}

.titlebar__right {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: 100%;
}

.titlebar__sep {
  width: 1px;
  height: 18px;
  background: var(--copper-border);
}

.titlebar__controls {
  display: flex;
  height: 100%;
}

.titlebar__btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 44px;
  height: 100%;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.titlebar__btn:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.titlebar__btn--close:hover {
  background: var(--copper-danger);
  color: #ffffff;
}
</style>
