<script setup lang="ts">
// 自定义标题栏：拖拽区域 + 窗口控制按钮 + 账户头像（紧贴控制按钮左侧）。

import { ref, onMounted, onUnmounted } from "vue";
import { Minus, Square, Copy, X } from "@lucide/vue";
import { getCurrentWindow } from "@tauri-apps/api/window";

import AccountMenu from "./AccountMenu.vue";
import { useI18n } from "../i18n";

const { t } = useI18n();

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
  <header class="titlebar" data-tauri-drag-region>
    <div class="titlebar__spacer" data-tauri-drag-region />
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
  justify-content: space-between;
  height: var(--copper-titlebar-h);
  flex-shrink: 0;
  background: var(--copper-bg);
}

.titlebar__spacer {
  flex: 1;
  height: 100%;
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
