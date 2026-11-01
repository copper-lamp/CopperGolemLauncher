<script setup lang="ts">
// 实例 · 屏幕触控层页（路由 `/version-settings/controls`，仅移动端入口）。
//
// 桌面端实例没有游戏 Activity，触控层没有落点；入口页也只在移动端给出这个动作。
// 页面只负责卡片与留白，编辑交给 `ControlsTab`（预览区、控件增删与保存都在里面）。

import { computed } from "vue";

import { useI18n } from "../../i18n";
import { usePlatform } from "../../composables/usePlatform";
import ControlsTab from "./ControlsTab.vue";
import { useVersionInstance } from "./useVersionInstance";

const { t } = useI18n();
const { platform } = usePlatform();
const { current, currentName } = useVersionInstance();

const title = computed(() => t("module.home.instance.page.controls"));
</script>

<template>
  <div class="controls-page">
    <section class="controls-page__panel">
      <header class="controls-page__header">
        <span class="controls-page__title">{{ title }}</span>
        <span v-if="current" class="controls-page__instance">{{ current.name }}</span>
      </header>
      <ControlsTab :name="currentName" :platform="platform" />
    </section>
  </div>
</template>

<style scoped>
.controls-page {
  height: 100%;
  min-height: 0;
  padding: var(--copper-space-4);
}

.controls-page__panel {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  overflow: hidden;
}

.controls-page__header {
  flex-shrink: 0;
  display: flex;
  align-items: baseline;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4);
  border-bottom: 1px solid var(--copper-border);
}

.controls-page__title {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
}

.controls-page__instance {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}
</style>
