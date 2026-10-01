<script setup lang="ts">
// 内核启动加载页：前后端能力就绪前展示的过渡界面。
//
// 不显示百分比：启动耗时不可预测（模块扫描 / 语言包加载），假进度条会误导用户，
// 因此用**不确定进度**的来回滑动细条表达「正在进行中」。
// 除应用名外不引入任何新 i18n 键 —— 内核语言包由后端维护，前端擅自加键会冲突；
// 下方提示文案来自内核语言包 `tips.items.*`，经 TipsRotator 自行取用。

import { useI18n } from "../i18n";
import TipsRotator from "./TipsRotator.vue";

const { t } = useI18n();
</script>

<template>
  <div class="loading-screen">
    <div class="loading-screen__content">
      <!-- 软件图标：与左导航栏共用 /app-64.png，保证品牌一致 -->
      <img
        class="loading-screen__logo"
        src="/app-64.png"
        width="56"
        height="56"
        alt=""
        aria-hidden="true"
      />

      <div class="loading-screen__name">{{ t("app.name") }}</div>

      <!-- 不确定进度条：轨道 + 循环滑动的滑块 -->
      <div class="loading-screen__bar">
        <div class="loading-screen__bar-inner"></div>
      </div>

      <TipsRotator :interval="6000" compact />
    </div>
  </div>
</template>

<style scoped>
.loading-screen {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  background: var(--copper-bg);
}

.loading-screen__content {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-5);
}

.loading-screen__logo {
  width: 56px;
  height: 56px;
  border-radius: var(--copper-radius-md);
  /* 轻微呼吸：表达进程存活，同时不喧宾夺主 */
  animation: logo-pulse 2.4s var(--copper-easing) infinite;
}

.loading-screen__name {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--copper-text);
}

.loading-screen__bar {
  position: relative;
  width: 180px;
  height: 2px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-3);
  overflow: hidden;
}

.loading-screen__bar-inner {
  position: absolute;
  top: 0;
  left: 0;
  width: 40%;
  height: 100%;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  animation: bar-sweep 1.4s var(--copper-easing) infinite;
}

/* 滑块左右往返：不确定进度的标准表达 */
@keyframes bar-sweep {
  0% {
    transform: translateX(-100%);
  }
  100% {
    transform: translateX(250%);
  }
}

@keyframes logo-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.65;
  }
}
</style>
