<script setup lang="ts">
// 启动 Tab：默认版本、内存分配、启动参数、显示日志、启动后行为。

import { computed } from "vue";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoSegmented from "../../components/ui/CoSegmented.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import CoSwitch from "../../components/ui/CoSwitch.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";

const { t } = useI18n();
const { get, set } = useSettings();

const defaultVersion = computed({
  get: () => get<string>("launch.default_version", ""),
  set: (value: string) => void set("launch.default_version", value),
});

const memoryMb = computed({
  get: () => get<number>("launch.memory_mb", 4096),
  set: (value: number) => void set("launch.memory_mb", value),
});

const args = computed({
  get: () => get<string>("launch.args", ""),
  set: (value: string) => void set("launch.args", value),
});

const showLogs = computed({
  get: () => get<boolean>("launch.show_logs", false),
  set: (value: boolean) => void set("launch.show_logs", value),
});

const afterLaunch = computed({
  get: () => get<string>("launch.after_launch", "keep"),
  set: (value: string) => void set("launch.after_launch", value),
});

const memoryOptions = [2048, 3072, 4096, 6144, 8192, 12288, 16384].map((mb) => ({
  value: String(mb),
  label: `${Math.round(mb / 1024)} GB (${mb} MB)`,
}));

const afterLaunchOptions = [
  { value: "keep", label: t("settings.launch.after_launch_keep") },
  { value: "minimize", label: t("settings.launch.after_launch_minimize") },
  { value: "hide", label: t("settings.launch.after_launch_hide") },
];

function updateMemory(value: string) {
  memoryMb.value = Number(value);
}
</script>

<template>
  <div class="launch-tab">
    <SettingSection title-key="settings.tabs.launch">
      <SettingRow label-key="settings.launch.default_version" hint-key="settings.launch.default_version_hint">
        <CoTextField
          :model-value="defaultVersion"
          :placeholder="t('settings.launch.default_version')"
          @update:model-value="defaultVersion = $event"
        />
      </SettingRow>
      <SettingRow label-key="settings.launch.memory" hint-key="settings.launch.memory_hint">
        <CoSelect
          :model-value="String(memoryMb)"
          :options="memoryOptions"
          @update:model-value="updateMemory"
        />
      </SettingRow>
      <SettingRow label-key="settings.launch.args">
        <CoTextField
          :model-value="args"
          :placeholder="t('settings.launch.args_placeholder')"
          @update:model-value="args = $event"
        />
      </SettingRow>
      <SettingRow label-key="settings.launch.show_logs">
        <CoSwitch :model-value="showLogs" @update:model-value="showLogs = $event" />
      </SettingRow>
      <SettingRow label-key="settings.launch.after_launch">
        <CoSegmented
          :model-value="afterLaunch"
          :options="afterLaunchOptions"
          @update:model-value="afterLaunch = $event"
        />
      </SettingRow>
    </SettingSection>
  </div>
</template>

<style scoped>
.launch-tab {
  max-width: 640px;
}
</style>
