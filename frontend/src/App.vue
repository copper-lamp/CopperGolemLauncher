<script setup lang="ts">
// 应用外壳：内核就绪前显示启动加载页（含随机提示），就绪后显示主界面。
//
// 加载页与主界面用 `v-if` 互斥而非叠放：主界面的各视图挂载即拉数据，
// 在内核未就绪时挂载会拿到一堆失败请求。

import TitleBar from "./components/TitleBar.vue";
import SideNav from "./components/SideNav.vue";
import ToastHost from "./components/ToastHost.vue";
import LoadingScreen from "./components/LoadingScreen.vue";
import { useKernelReady } from "./composables/useKernelReady";

const { isKernelReady } = useKernelReady();
</script>

<template>
  <!-- 加载页占满整个窗口（含标题栏区域），此时窗口尚无内容可拖拽 -->
  <LoadingScreen v-if="!isKernelReady" />

  <div v-else class="app-shell">
    <SideNav />
    <div class="app-main">
      <TitleBar />
      <main class="app-content">
        <RouterView />
      </main>
    </div>
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
</style>
