<script setup lang="ts">
// 设置页：左上角标题 + 固定 Tabs（通用 / 版本 / 启动 / 个性 / 模块 / 关于）。

import { ref } from "vue";

import GeneralTab from "./settings/GeneralTab.vue";
import VersionTab from "./settings/VersionTab.vue";
import LaunchTab from "./settings/LaunchTab.vue";
import AppearanceTab from "./settings/AppearanceTab.vue";
import ModulesTab from "./settings/ModulesTab.vue";
import AboutTab from "./settings/AboutTab.vue";
import { useI18n } from "../i18n";

const { t } = useI18n();

const tabs = [
  { id: "general", titleKey: "settings.tabs.general" },
  { id: "version", titleKey: "settings.tabs.version" },
  { id: "launch", titleKey: "settings.tabs.launch" },
  { id: "appearance", titleKey: "settings.tabs.appearance" },
  { id: "modules", titleKey: "settings.tabs.modules" },
  { id: "about", titleKey: "settings.tabs.about" },
] as const;

const active = ref<(typeof tabs)[number]["id"]>("general");
</script>

<template>
  <div class="settings">
    <nav class="settings__tabs">
      <button
        v-for="tab in tabs"
        :key="tab.id"
        :class="['settings__tab', { 'settings__tab--active': active === tab.id }]"
        @click="active = tab.id"
      >
        {{ t(tab.titleKey) }}
      </button>
    </nav>

    <div class="settings__body">
      <GeneralTab v-if="active === 'general'" />
      <VersionTab v-else-if="active === 'version'" />
      <LaunchTab v-else-if="active === 'launch'" />
      <AppearanceTab v-else-if="active === 'appearance'" />
      <ModulesTab v-else-if="active === 'modules'" />
      <AboutTab v-else />
    </div>
  </div>
</template>

<style scoped>
.settings {
  height: 100%;
  display: flex;
  flex-direction: column;
}

.settings__tabs {
  display: flex;
  gap: var(--copper-space-1);
  padding: var(--copper-space-5) var(--copper-space-6) 0;
  border-bottom: 1px solid var(--copper-border);
}

.settings__tab {
  position: relative;
  height: 36px;
  padding: 0 var(--copper-space-4);
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-md);
  cursor: pointer;
  transition: color var(--copper-duration-fast) var(--copper-easing);
}

.settings__tab:hover {
  color: var(--copper-text);
}

.settings__tab--active {
  color: var(--copper-accent);
  font-weight: 600;
}

.settings__tab--active::after {
  content: "";
  position: absolute;
  left: var(--copper-space-2);
  right: var(--copper-space-2);
  bottom: -1px;
  height: 2px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
}

.settings__body {
  flex: 1;
  padding: var(--copper-space-5) var(--copper-space-6);
  overflow-y: auto;
}
</style>
