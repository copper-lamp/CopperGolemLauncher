<script setup lang="ts">
// 应用外壳：内核就绪前显示启动加载页（含随机提示），就绪后按平台形态渲染。
//
// 加载页与主界面用 `v-if` 互斥而非叠放：主界面的各视图挂载即拉数据，
// 在内核未就绪时挂载会拿到一堆失败请求。
//
// 两态布局（见 docs/平台适配.md 2.5）：
// - 桌面：左导航 + 标题栏（含窗口控件）+ 内容区；
// - 移动：标题栏（无窗口控件，无左导航）+ 内容区 + 底部标签栏。
// 内容区与页面组件完全共用，仅外壳取向不同。

import TitleBar from "./components/TitleBar.vue";
import SideNav from "./components/SideNav.vue";
import MobileNav from "./components/MobileNav.vue";
import ToastHost from "./components/ToastHost.vue";
import LoadingScreen from "./components/LoadingScreen.vue";
import { useKernelReady } from "./composables/useKernelReady";
import { usePlatform } from "./composables/usePlatform";

const { isKernelReady } = useKernelReady();
const { isMobile } = usePlatform();
</script>

<template>
  <!-- 加载页占满整个窗口（含标题栏区域），此时窗口尚无内容可拖拽 -->
  <LoadingScreen v-if="!isKernelReady" />

  <!-- 桌面：左导航 + 标题栏 + 内容区 -->
  <div v-else-if="!isMobile" class="app-shell">
    <SideNav />
    <div class="app-main">
      <TitleBar />
      <main class="app-content">
        <RouterView />
      </main>
    </div>
    <ToastHost />
  </div>

  <!-- 移动：标题栏 + 内容区 + 底部标签栏 -->
  <div v-else class="app-shell app-shell--mobile">
    <div class="app-main">
      <TitleBar />
      <main class="app-content">
        <RouterView />
      </main>
    </div>
    <MobileNav />
    <ToastHost />
  </div>
</template>

<style scoped>
.app-main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.app-content {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

/* 移动端为纵向：内容 + 底部标签栏上下排布。 */
.app-shell--mobile {
  flex-direction: column;
}
</style>
