<script setup lang="ts">
// 个性 Tab：列表密度、动画开关、下载悬浮窗自动隐藏、渲染性能偏好。

import { computed } from "vue";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoSegmented from "../../components/ui/CoSegmented.vue";
import CoSwitch from "../../components/ui/CoSwitch.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";

const { t } = useI18n();
const { get, set } = useSettings();

const density = computed({
  get: () => get<string>("appearance.list_density", "comfortable"),
  set: (value: string) => void set("appearance.list_density", value),
});

const animations = computed({
  get: () => get<boolean>("appearance.animations", true),
  set: (value: boolean) => void set("appearance.animations", value),
});

const overlayAutoHide = computed({
  get: () => get<boolean>("download_overlay.auto_hide", true),
  set: (value: boolean) => void set("download_overlay.auto_hide", value),
});

const render = computed({
  get: () => get<string>("performance.render", "balanced"),
  set: (value: string) => void set("performance.render", value),
});

const densityOptions = [
  { value: "cozy", label: t("settings.appearance.list_density_cozy") },
  { value: "comfortable", label: t("settings.appearance.list_density_comfortable") },
  { value: "compact", label: t("settings.appearance.list_density_compact") },
];

const renderOptions = [
  { value: "performance", label: t("settings.appearance.render_performance") },
  { value: "balanced", label: t("settings.appearance.render_balanced") },
  { value: "quality", label: t("settings.appearance.render_quality") },
];
</script>

<template>
  <div class="appearance-tab">
    <SettingSection title-key="settings.tabs.appearance">
      <SettingRow label-key="settings.appearance.list_density">
        <CoSegmented :model-value="density" :options="densityOptions" @update:model-value="density = $event" />
      </SettingRow>
      <SettingRow label-key="settings.appearance.animations">
        <CoSwitch :model-value="animations" @update:model-value="animations = $event" />
      </SettingRow>
      <SettingRow label-key="settings.appearance.download_overlay_auto_hide">
        <CoSwitch :model-value="overlayAutoHide" @update:model-value="overlayAutoHide = $event" />
      </SettingRow>
      <SettingRow label-key="settings.appearance.render">
        <CoSegmented :model-value="render" :options="renderOptions" @update:model-value="render = $event" />
      </SettingRow>
    </SettingSection>
  </div>
</template>

<style scoped>
.appearance-tab {
  max-width: 640px;
}
</style>
