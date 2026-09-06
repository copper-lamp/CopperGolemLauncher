<script setup lang="ts">
// 通用 Tab：语言、外观深浅色、强调色（预设 / 自定义）。

import { ref, computed } from "vue";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoSegmented from "../../components/ui/CoSegmented.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { themeSetAccent } from "../../api/theme";

const { t, locale, setLocale, supportedLocales } = useI18n();
const { get, set } = useSettings();

const mode = computed({
  get: () => get<string>("theme.mode", "auto"),
  set: (value: string) => void set("theme.mode", value),
});

const accent = ref(get<string>("theme.accent", "#3b82f6"));

const localeOptions = supportedLocales().map((code) => ({ value: code, label: code }));

const modeOptions = [
  { value: "auto", label: t("settings.general.theme_mode_auto") },
  { value: "light", label: t("settings.general.theme_mode_light") },
  { value: "dark", label: t("settings.general.theme_mode_dark") },
];

const presets = [
  "#3b82f6",
  "#10b981",
  "#f59e0b",
  "#ef4444",
  "#8b5cf6",
  "#ec4899",
  "#0ea5e9",
  "#14b8a6",
];

async function changeLocale(code: string) {
  if (code === locale.value) return;
  await setLocale(code);
  showToast(t("common.success"), "success");
}

async function applyAccent(hex: string) {
  if (!/^#[0-9a-fA-F]{6}$/.test(hex)) {
    showToast(t("toast.error", { message: hex }), "error");
    return;
  }
  accent.value = hex;
  try {
    await themeSetAccent(hex);
  } catch (e) {
    showToast(String(e), "error");
  }
}
</script>

<template>
  <div class="general-tab">
    <SettingSection title-key="settings.general">
      <SettingRow label-key="settings.general.language" hint-key="settings.general.language_hint">
        <CoSelect
          :model-value="locale"
          :options="localeOptions"
          @update:model-value="changeLocale"
        />
      </SettingRow>
      <SettingRow label-key="settings.general.theme_mode">
        <CoSegmented :model-value="mode" :options="modeOptions" @update:model-value="mode = $event" />
      </SettingRow>
    </SettingSection>

    <SettingSection title-key="settings.general.accent_preset">
      <SettingRow label-key="settings.general.accent_color">
        <div class="general-tab__accent">
          <button
            v-for="preset in presets"
            :key="preset"
            :class="['general-tab__swatch', { 'general-tab__swatch--active': accent === preset }]"
            :style="{ background: preset }"
            :title="preset"
            @click="applyAccent(preset)"
          />
          <CoTextField
            :model-value="accent"
            class="general-tab__accent-input"
            @update:model-value="accent = $event"
            @enter="applyAccent(accent)"
            @blur="applyAccent(accent)"
          />
        </div>
      </SettingRow>
    </SettingSection>
  </div>
</template>

<style scoped>
.general-tab {
  max-width: 640px;
}

.general-tab__accent {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.general-tab__swatch {
  width: 26px;
  height: 26px;
  border: 2px solid transparent;
  border-radius: var(--copper-radius-full);
  cursor: pointer;
  transition:
    transform var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing);
}

.general-tab__swatch:hover {
  transform: scale(1.12);
}

.general-tab__swatch--active {
  border-color: var(--copper-text);
  transform: scale(1.08);
}

.general-tab__accent-input {
  width: 96px;
}
</style>
