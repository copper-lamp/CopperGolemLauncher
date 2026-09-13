<script setup lang="ts">
// 自定义标题栏：左侧为「返回按钮 + 当前页面标题」，中部拖拽区，右侧为页面注入的操作区
// + 账户头像 + 窗口控制按钮。标题/返回来自当前路由 meta，操作区由各页面经 Teleport 注入。

import { ref, computed, onMounted, onUnmounted } from "vue";
import { Minus, Square, Copy, X, ArrowLeft } from "@lucide/vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useRoute, useRouter } from "vue-router";

import AccountMenu from "./AccountMenu.vue";
import { useI18n } from "../i18n";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();

/** 当前路由标题（i18n）与返回目标路径。 */
const title = computed(() => {
  const key = route.meta.titleKey;
  return typeof key === "string" ? t(key) : "";
});
const backPath = computed(() =>
  typeof route.meta.backPath === "string" ? route.meta.backPath : null,
);

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
      <button
        v-if="backPath"
        class="titlebar__back"
        :title="t('titlebar.back')"
        @click="goBack"
      >
        <ArrowLeft :size="15" />
      </button>
      <h1 v-if="title" class="titlebar__title">{{ title }}</h1>
    </div>
    <div class="titlebar__spacer" data-tauri-drag-region />
    <!-- 页面注入操作区：各模块经 <Teleport to="#copper-titlebar-actions"> 放置按钮/切换。 -->
    <div class="titlebar__actions" id="copper-titlebar-actions" />
    <div class="titlebar__right">
      <AccountMenu />
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
