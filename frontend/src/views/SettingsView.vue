<script setup lang="ts">
// 设置页（V3 骨架的第一批消费者）：左侧分类导航 + 右侧设置内容。
//
// 版面约定（见 docs/铜核心/界面布局.md）：
// - 页面留白与滚动由 CoPage 统一承担，本页不再自己写 padding / overflow；
// - 左右分栏由 CoSplitLayout 承担：分类导航作为左副栏，窄容器时自动落为上下堆叠；
// - 分类导航吸顶，滚动右侧长内容（如 LLM 模型表）时仍能切换分类；
// - 选中态沿用全站强调色语言（与 SideNav 一致），视觉细化不改变结构。
//
// 为什么先改设置页：它是「左导航 + 右内容」最纯粹的一屏，不依赖模块数据，
// 用它验证分栏原语的吸顶、折叠与滚动归属，风险最低。

import { ref } from "vue";
import { SlidersHorizontal, FolderOpen, Rocket, Palette, Blocks, Info, Download, Languages, Puzzle, ShieldCheck, RefreshCw } from "@lucide/vue";

import GeneralTab from "./settings/GeneralTab.vue";
import StorageTab from "./settings/StorageTab.vue";
import DownloadTab from "./settings/DownloadTab.vue";
import TranslationTab from "./settings/TranslationTab.vue";
import UpdateTab from "./settings/UpdateTab.vue";
import PrivacyTab from "./settings/PrivacyTab.vue";
import SettingsContributions from "./settings/SettingsContributions.vue";
import { useSettingsText } from '../services/settings-categories';
const st = useSettingsText();
import LaunchTab from "./settings/LaunchTab.vue";
import AppearanceTab from "./settings/AppearanceTab.vue";
import ModulesTab from "./settings/ModulesTab.vue";
import ComponentsTab from "./settings/ComponentsTab.vue";
import AboutTab from "./settings/AboutTab.vue";
import CoPage from "../components/layout/CoPage.vue";
import CoSkeleton from '../components/ui/CoSkeleton.vue';
import {useSettings} from '../composables/useSettings';
const {loaded}=useSettings();
import CoSplitLayout from "../components/layout/CoSplitLayout.vue";
import { useI18n } from "../i18n";

const { t } = useI18n();

/** 分类导航：图标与文案一一对应，文案仍走 i18n，不写死。 */
const tabs = [
  { id: "general", titleKey: "settings.tabs.general", icon: SlidersHorizontal },
  { id: "appearance", titleKey: "settings.tabs.appearance", icon: Palette },
  { id: "storage", titleKey: "settings.tabs.version", icon: FolderOpen },
  { id: "download", titleKey: "settings.tabs.general", icon: Download },
  { id: "launch", titleKey: "settings.tabs.launch", icon: Rocket },
  { id: "translation", titleKey: "settings.tabs.general", icon: Languages },
  { id: "modules", titleKey: "settings.tabs.modules", icon: Blocks },
  { id: "components", titleKey: "components.title", icon: Puzzle },
  { id: "privacy", titleKey: "settings.tabs.general", icon: ShieldCheck },
  { id: "update", titleKey: "settings.tabs.about", icon: RefreshCw },
  { id: "about", titleKey: "settings.tabs.about", icon: Info },
] as const;

const active = ref<(typeof tabs)[number]["id"]>("general");
function navigateTab(event:KeyboardEvent,id:(typeof tabs)[number]['id']){const index=tabs.findIndex(tab=>tab.id===id);const next=event.key==='Home'?0:event.key==='End'?tabs.length-1:['ArrowDown','ArrowRight'].includes(event.key)?(index+1)%tabs.length:['ArrowUp','ArrowLeft'].includes(event.key)?(index+tabs.length-1)%tabs.length:-1;if(next<0)return;event.preventDefault();active.value=tabs[next]!.id;const buttons=(event.currentTarget as HTMLElement).parentElement?.querySelectorAll<HTMLButtonElement>('[role=tab]');buttons?.[next]?.focus();}
</script>

<template>
  <CoPage>
    <CoSplitLayout aside-position="start" aside-width="var(--copper-subnav-w)">
      <div class="settings__body">
        <CoSkeleton v-if="!loaded" variant="settings" :label="t('common.loading')" />
        <GeneralTab v-else-if="active === 'general'" />
        <StorageTab v-else-if="active === 'storage'" />
        <DownloadTab v-else-if="active === 'download'" />
        <TranslationTab v-else-if="active === 'translation'" />
        <PrivacyTab v-else-if="active === 'privacy'" />
        <UpdateTab v-else-if="active === 'update'" />
        <LaunchTab v-else-if="active === 'launch'" />
        <AppearanceTab v-else-if="active === 'appearance'" />
        <ModulesTab v-else-if="active === 'modules'" />
        <ComponentsTab v-else-if="active === 'components'" />
        <AboutTab v-else />
        <SettingsContributions :category="active" />
      </div>

      <template #aside>
        <nav class="settings__nav" role="tablist" :aria-label="t('settings.title')">
          <button
            v-for="tab in tabs"
            :key="tab.id"
            type="button"
            role="tab"
            :aria-selected="active === tab.id"
            :tabindex="active === tab.id ? 0 : -1"
            @keydown="navigateTab($event,tab.id)"
            :class="['settings__nav-item', { 'settings__nav-item--active': active === tab.id }]"
            @click="active = tab.id"
          >
            <component :is="tab.icon" :size="15" class="settings__nav-icon" />
            <span class="settings__nav-label">{{ st(tab.id) }}</span>
          </button>
        </nav>
      </template>
    </CoSplitLayout>
  </CoPage>
</template>

<style scoped>
.settings__body {
  min-width: 0;
}

.settings__nav {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.settings__nav-item {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  width: 100%;
  height: 36px;
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.settings__nav-item:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

/* 选中态：浅强调色底 + 强调色字，与 SideNav 的当前项使用同一套语言。 */
.settings__nav-item--active {
  background: color-mix(in srgb, var(--copper-accent) 16%, transparent);
  color: var(--copper-accent);
  font-weight: 600;
}

.settings__nav-icon {
  flex-shrink: 0;
}

.settings__nav-label {
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

/* 分栏折叠后导航横排：竖排六项会在正文之前占掉一整屏高度。
 * 容器查询的参照是 CoSplitLayout 建立的内容区容器，不是窗口。 */
@container (max-width: 760px) {
  .settings__nav {
    flex-direction: row;
    flex-wrap: wrap;
  }

  .settings__nav-item {
    width: auto;
    flex: 0 0 auto;
  }
}
</style>