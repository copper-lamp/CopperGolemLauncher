<script setup lang="ts">
// 应用外壳：左导航 + 右侧（标题栏 + 内容区）+ Toast 宿主。

import TitleBar from "./components/TitleBar.vue";
import SideNav from "./components/SideNav.vue";
import ToastHost from "./components/ToastHost.vue";
</script>

<template>
  <div class="app-shell">
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
