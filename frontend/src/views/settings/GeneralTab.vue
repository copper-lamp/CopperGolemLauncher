<script setup lang="ts">
// 通用 Tab：语言、外观深浅色、强调色（预设 / 自定义）、LLM 配置。

import { computed, onMounted, ref } from "vue";
import { Eye, EyeOff } from "@lucide/vue";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoSegmented from "../../components/ui/CoSegmented.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { useSettings } from "../../composables/useSettings";
import { llmClearApiKey, llmSaveConfig, llmSetApiKey, llmStatus } from "../../api/llm";
import { themeState, setThemeMode, setAccent, type ThemeMode } from "../../theme";

const { t, locale, setLocale, supportedLocales } = useI18n();
const { get, set } = useSettings();

const mode = computed({
  get: () => themeState.mode,
  set: (value: string) => void setThemeMode(value as ThemeMode),
});

const accent = computed({
  get: () => themeState.accent,
  set: (value: string) => {
    themeState.accent = value;
  },
});

const localeOptions = supportedLocales().map((code) => ({ value: code, label: code }));

const modeOptions = [
  { value: "auto", label: t("settings.general.theme_mode_auto") },
  { value: "light", label: t("settings.general.theme_mode_light") },
  { value: "dark", label: t("settings.general.theme_mode_dark") },
];

const presets = [
  "#c97b3d",
  "#b87333",
  "#d9a05b",
  "#e8b36a",
  "#a8542a",
  "#58a6ff",
  "#3fb950",
  "#d29922",
  "#f85149",
  "#8957e5",
];

// 下载页「历史下载」展示上限。0 = 不显示；数值取值为固定档位，避免自由输入产生
// 无意义的展示窗口。仅影响显示，不删除任何记录（历史记录本身只在内存中）。
const historyLimit = computed({
  get: () => get<number>("download.history_limit", 50),
  set: (value: number) => void set("download.history_limit", value),
});

const historyLimitOptions = [0, 20, 50, 70, 100, 150, 200].map((n) => ({
  value: String(n),
  label: n === 0 ? t("settings.general.download_history_none") : t(`settings.general.download_history_${n}`),
}));

function updateHistoryLimit(value: string) {
  historyLimit.value = Number(value);
}

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
  await setAccent(hex);
}

// ------------------------------------------------ LLM 配置（内核唯一的 AI 接入读取点）
// 三项：base URL / 模型名 / API Key。密钥经密钥环存储且永不回显，仅展示「是否已配置」。
const llmBaseUrl = ref("");
const llmModel = ref("");
const llmApiKey = ref("");
const llmApiKeyConfigured = ref(false);
const llmShowKey = ref(false);
const llmSaving = ref(false);

const llmKeyStatus = computed(() =>
  llmApiKeyConfigured.value
    ? t("settings.general.llm_configured")
    : t("settings.general.llm_not_configured"),
);

async function loadLlmStatus() {
  try {
    const status = await llmStatus();
    llmBaseUrl.value = status.base_url;
    llmModel.value = status.model;
    llmApiKeyConfigured.value = status.api_key_configured;
    // 密钥不回填：清空输入框，避免以任何形式把已存密钥带进界面。
    llmApiKey.value = "";
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function saveLlm() {
  llmSaving.value = true;
  try {
    await llmSaveConfig(llmBaseUrl.value, llmModel.value);
    if (llmApiKey.value.trim()) {
      await llmSetApiKey(llmApiKey.value);
    }
    await loadLlmStatus();
    showToast(t("settings.general.llm_saved"), "success");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    llmSaving.value = false;
  }
}

async function clearLlmKey() {
  try {
    await llmClearApiKey();
    await loadLlmStatus();
  } catch (e) {
    showToast(String(e), "error");
  }
}

onMounted(loadLlmStatus);
</script>

<template>
  <div class="general-tab">
    <SettingSection title-key="settings.tabs.general">
      <SettingRow label-key="settings.general.language" hint-key="settings.general.language_hint">
        <CoSelect
          :model-value="locale"
          :options="localeOptions"
          @update:model-value="changeLocale"
        />
      </SettingRow>
      <SettingRow label-key="settings.general.theme_mode">
        <CoSegmented :model-value="mode" :options="modeOptions" @update:model-value="mode = $event as ThemeMode" />
      </SettingRow>
      <SettingRow
        label-key="settings.general.download_history"
        hint-key="settings.general.download_history_hint"
      >
        <CoSelect
          :model-value="String(historyLimit)"
          :options="historyLimitOptions"
          @update:model-value="updateHistoryLimit"
        />
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

    <SettingSection title-key="settings.general.llm">
      <SettingRow label-key="settings.general.llm_base_url" hint-key="settings.general.llm_hint">
        <CoTextField
          :model-value="llmBaseUrl"
          class="general-tab__llm-field"
          :placeholder="t('settings.general.llm_base_url_placeholder')"
          @update:model-value="llmBaseUrl = $event"
        />
      </SettingRow>
      <SettingRow label-key="settings.general.llm_model">
        <CoTextField
          :model-value="llmModel"
          class="general-tab__llm-field"
          :placeholder="t('settings.general.llm_model_placeholder')"
          @update:model-value="llmModel = $event"
        />
      </SettingRow>
      <SettingRow label-key="settings.general.llm_api_key" hint-key="settings.general.llm_api_key_hint">
        <div class="general-tab__llm-key">
          <CoTextField
            :model-value="llmApiKey"
            :type="llmShowKey ? 'text' : 'password'"
            @update:model-value="llmApiKey = $event"
            @enter="saveLlm"
          />
          <CoButton
            variant="ghost"
            size="sm"
            :title="t('settings.general.llm_api_key')"
            @click="llmShowKey = !llmShowKey"
          >
            <EyeOff v-if="llmShowKey" :size="16" />
            <Eye v-else :size="16" />
          </CoButton>
          <span class="general-tab__llm-status">{{ llmKeyStatus }}</span>
        </div>
      </SettingRow>
      <div class="general-tab__llm-actions">
        <CoButton variant="primary" size="sm" :disabled="llmSaving" @click="saveLlm">
          {{ t("settings.general.llm_save") }}
        </CoButton>
        <CoButton variant="danger" size="sm" :disabled="!llmApiKeyConfigured" @click="clearLlmKey">
          {{ t("settings.general.llm_clear") }}
        </CoButton>
      </div>
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

.general-tab__llm-field {
  width: 320px;
}

.general-tab__llm-key {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  width: 380px;
}

.general-tab__llm-status {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
}

.general-tab__llm-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--copper-space-2);
  padding: var(--copper-space-2) 0;
}
</style>
