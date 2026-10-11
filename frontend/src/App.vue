<script setup lang="ts">
// 应用外壳：内核就绪前显示启动加载页（含随机提示），就绪后按平台形态渲染。
//
// 加载页与主界面用 `v-if` 互斥而非叠放：主界面的各视图挂载即拉数据，
// 在内核未就绪时挂载会拿到一堆失败请求。
//
// 两态布局（见 docs/平台适配.md 2.5）：
// - 桌面 / 移动横屏：左导航（单 icon）+ 标题栏 + 内容区；
// - 移动竖屏：标题栏（无窗口控件，无左导航）+ 内容区 + 底部标签栏。
// 内容区与页面组件完全共用，仅外壳取向不同。
//
// 为什么移动横屏不用底部栏而直接复用桌面左导航：
// 1. 安卓端已锁定横屏（见 AndroidManifest.xml），横屏是移动端唯一的落点；
// 2. 竖屏的「纵向 flex 外壳」会让内容区的高度由 flex-grow 决定，而 Chromium
//    不把它当作**确定高度**，页面根的 `height:100%` 于是退化成 auto，滚动容器
//    永远撑到内容高度、无法滚动。桌面「横向外壳 + align-items:stretch」给的是
//    确定高度，滚动链路才成立。复用同一套外壳顺带消掉了这个坑（真机实测）。

import TitleBar from "./components/TitleBar.vue";
import SideNav from "./components/SideNav.vue";
import MobileNav from "./components/MobileNav.vue";
import ToastHost from "./components/ToastHost.vue";
import LoadingScreen from "./components/LoadingScreen.vue";
import UpdateDialog from "./components/update/UpdateDialog.vue";
import DiagnosticOverlay from "./components/DiagnosticOverlay.vue";
import { useKernelReady } from "./composables/useKernelReady";
import { usePlatform } from "./composables/usePlatform";

const { isKernelReady } = useKernelReady();
const { isMobile, isLandscape } = usePlatform();
</script>

<template>
  <!-- 加载页占满整个窗口（含标题栏区域），此时窗口尚无内容可拖拽 -->
  <LoadingScreen v-if="!isKernelReady" />

  <!-- 桌面 / 移动横屏：左导航 + 标题栏 + 内容区（同一套外壳） -->
  <div v-else-if="!isMobile || isLandscape" class="app-shell">
    <SideNav />
    <div class="app-main">
      <TitleBar />
      <main class="app-content">
        <RouterView />
      </main>
    </div>
    <ToastHost />
  </div>

  <!-- 移动竖屏：标题栏 + 内容区 + 底部标签栏 -->
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

  <!-- 更新弹窗：与两态布局平级，仅在用户点击标题栏的更新按钮后出现 -->
  <UpdateDialog />

  <!-- 启动诊断面板：覆盖在最上层，引导失败时自动出现 -->
  <DiagnosticOverlay />
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