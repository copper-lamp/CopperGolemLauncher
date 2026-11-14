<script setup lang="ts">
// 启动 Tab：默认版本、启动参数、环境变量、启动后行为、游戏退出后。
//
// 不提供“内存分配”与“显示日志”：这两个选项对基岩版没有实际作用，
// 留在界面上只会让用户以为它们生效（假实现比缺功能更糟）。

import { computed, onMounted, ref } from "vue";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoSegmented from "../../components/ui/CoSegmented.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { homeVersionsList, type VersionView } from "../../api/home";

const { t } = useI18n();
const { get, set } = useSettings();

const versions = ref<VersionView[]>([]);

const defaultVersion = computed({
  get: () => get<string>("launch.default_version", ""),
  set: (value: string) => void set("launch.default_version", value),
});

// 下拉而非文本框：这个值同时是内容下载的落点目标，手打错一个字符不会报错，
// 只会让内容静默落到系统下载目录。枚举已装版本从根上消除这种输入。
const versionOptions = computed(() => [
  { value: "", label: t("settings.launch.default_version_none") },
  ...versions.value.map((v) => ({ value: v.name, label: v.name })),
]);

onMounted(async () => {
  try {
    versions.value = await homeVersionsList();
  } catch (e) {
    showToast(String(e), "error");
  }
});

const args = computed({
  get: () => get<string>("launch.args", ""),
  set: (value: string) => void set("launch.args", value),
});

const envVars = computed({
  get: () => get<string>("launch.env_vars", ""),
  set: (value: string) => void set("launch.env_vars", value),
});

const afterLaunch = computed({
  get: () => get<string>("launch.after_launch", "keep"),
  set: (value: string) => void set("launch.after_launch", value),
});

const afterGameExit = computed({
  get: () => get<string>("launch.after_game_exit", "keep"),
  set: (value: string) => void set("launch.after_game_exit", value),
});

const afterLaunchOptions = computed(() => [
  { value: "keep", label: t("settings.launch.after_launch_keep") },
  { value: "minimize", label: t("settings.launch.after_launch_minimize") },
  { value: "hide", label: t("settings.launch.after_launch_hide") },
]);

const afterGameExitOptions = computed(() => [
  { value: "keep", label: t("settings.launch.after_exit_keep") },
  { value: "reopen", label: t("settings.launch.after_exit_reopen") },
  { value: "close", label: t("settings.launch.after_exit_close") },
]);
</script>

<template>
  <div class="launch-tab">
    <SettingSection title-key="settings.tabs.launch">
      <SettingRow label-key="settings.launch.default_version" hint-key="settings.launch.default_version_hint">
        <CoSelect
          :model-value="defaultVersion"
          :options="versionOptions"
          @update:model-value="defaultVersion = $event"
        />
      </SettingRow>
      <SettingRow label-key="settings.launch.args" hint-key="settings.launch.args_hint">
        <CoTextField
          class="launch-tab__field"
          :model-value="args"
          :placeholder="t('settings.launch.args_placeholder')"
          @update:model-value="args = $event"
        />
      </SettingRow>
      <SettingRow label-key="settings.launch.environment" hint-key="settings.launch.environment_hint">
        <textarea
          class="launch-tab__env"
          :value="envVars"
          :placeholder="t('settings.launch.environment_placeholder')"
          spellcheck="false"
          rows="4"
          @input="envVars = ($event.target as HTMLTextAreaElement).value"
        />
      </SettingRow>
      <SettingRow
        label-key="settings.launch.after_launch"
        hint-key="settings.launch.after_launch_hint"
      >
        <CoSegmented
          :model-value="afterLaunch"
          :options="afterLaunchOptions"
          @update:model-value="afterLaunch = $event"
        />
      </SettingRow>
      <SettingRow
        label-key="settings.launch.after_game_exit"
        hint-key="settings.launch.after_game_exit_hint"
      >
        <CoSegmented
          :model-value="afterGameExit"
          :options="afterGameExitOptions"
          @update:model-value="afterGameExit = $event"
        />
      </SettingRow>
    </SettingSection>
  </div>
</template>

<style scoped>
.launch-tab {
  max-width: 640px;
}

.launch-tab__field {
  width: min(360px, 46vw);
}

.launch-tab__env {
  width: min(420px, 52vw);
  min-height: calc(var(--copper-control-h) * 2.2);
  padding: var(--copper-space-3);
  border: 0;
  border-radius: var(--copper-radius-control);
  background: var(--copper-surface-2);
  color: var(--copper-text);
  font-family: var(--copper-font-mono, ui-monospace, SFMono-Regular, Consolas, monospace);
  font-size: var(--copper-font-size-sm);
  line-height: 1.5;
  resize: vertical;
  box-shadow: var(--copper-shadow-subtle);
  transition: background var(--copper-duration-fast);
}

.launch-tab__env::placeholder {
  color: var(--copper-text-disabled);
}

.launch-tab__env:hover:not(:disabled) {
  background: var(--copper-surface-3);
}

.launch-tab__env:focus-visible {
  outline: 2px solid var(--copper-accent);
  outline-offset: 2px;
}
</style>
