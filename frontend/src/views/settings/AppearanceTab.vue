<script setup lang="ts">
// 个性化 Tab：主题（骨架屏预览卡）、强调色（色板 + 取色器）、动画偏好。
//
// 主题卡不是单选标签：每张卡都用迷你骨架屏画出该主题下的真实层次
// （侧栏 / 标题 / 内容块），深色与浅色并排放，时间与跟随系统用双拼预览，
// 用户不必点进去才知道长什么样。
//
// 预览色是固定的设计常量，不能读当前主题令牌：预览需要同时展示另一套主题，
// 而令牌只有“当前”这一份。改主题令牌时需同步这里的四个预览色。

import { computed } from "vue";
import { Check, Monitor, Moon, Palette, Sun, SunMoon } from "@lucide/vue";

import SettingSection from "./SettingSection.vue";
import CoSegmented from "../../components/ui/CoSegmented.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";
import { themeState, setThemeMode, setAccent, type ThemeMode } from "../../theme";

const { t } = useI18n();
const { get, set } = useSettings();

const modes = computed(() =>
  [
    { value: "dark" as ThemeMode, icon: Moon },
    { value: "light" as ThemeMode, icon: Sun },
    { value: "time" as ThemeMode, icon: SunMoon },
    { value: "auto" as ThemeMode, icon: Monitor },
  ].map((option) => ({ ...option, label: t("settings.general.theme_mode_" + option.value) })),
);

const mode = computed({
  get: () => themeState.mode,
  set: (value: ThemeMode) => void setThemeMode(value),
});

/** 预览里要画几块面板：深色 / 浅色各一块，时间与跟随系统双拼。 */
function panes(value: ThemeMode): Array<"dark" | "light"> {
  if (value === "dark") return ["dark"];
  if (value === "light") return ["light"];
  return ["light", "dark"];
}

const motion = computed({
  get: () => get<string>("appearance.motion_preset", "smooth"),
  set: (value: string) => void set("appearance.motion_preset", value),
});

const motionOptions = computed(() =>
  ["fast", "smooth", "silky"].map((value) => ({
    value,
    label: t("settings.appearance.motion_" + value),
  })),
);

/** 预设色板：覆盖常见的暖铜、冷色与低饱和，最后一格是自由取色。 */
const swatches = [
  "#c97b3d",
  "#d9a441",
  "#c2554f",
  "#b8557f",
  "#8d6bd1",
  "#5b7fd4",
  "#3f9bc4",
  "#43a08a",
  "#5f9c4a",
  "#7c8794",
];

function normalize(hex: string): string {
  return hex.toLowerCase();
}

/** 色块上的对勾颜色：按亮度选黑/白，浅色块上白勾会看不见。 */
function swatchInk(hex: string): string {
  const value = hex.replace("#", "");
  const r = parseInt(value.slice(0, 2), 16);
  const g = parseInt(value.slice(2, 4), 16);
  const b = parseInt(value.slice(4, 6), 16);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 150 ? "#101418" : "#ffffff";
}

function applyAccent(value: string): void {
  if (/^#[0-9a-f]{6}$/i.test(value)) void setAccent(value);
}

function pickAccent(event: Event): void {
  const input = event.target as HTMLInputElement;
  applyAccent(input.value);
}
</script>

<template>
  <div class="appearance-tab">
    <SettingSection title-key="settings.appearance.theme_section">
      <div class="theme-grid" role="radiogroup" :aria-label="t('settings.general.theme_mode')">
        <button
          v-for="option in modes"
          :key="option.value"
          type="button"
          role="radio"
          :aria-checked="mode === option.value"
          :class="['theme-card', { 'theme-card--active': mode === option.value }]"
          @click="mode = option.value"
        >
          <div class="theme-card__preview">
            <div
              v-for="pane in panes(option.value)"
              :key="pane"
              :class="['preview-pane', 'preview-pane--' + pane]"
            >
              <span class="preview-pane__rail" />
              <span class="preview-pane__title" />
              <span class="preview-pane__line" />
              <span class="preview-pane__line preview-pane__line--short" />
              <span class="preview-pane__chip" />
            </div>
            <span class="theme-card__badge">
              <component :is="option.icon" :size="12" />
            </span>
            <span v-if="mode === option.value" class="theme-card__check">
              <Check :size="12" />
            </span>
          </div>
          <span class="theme-card__label">{{ option.label }}</span>
        </button>
      </div>
      <p class="appearance-tab__hint">{{ t("settings.appearance.theme_hint") }}</p>
    </SettingSection>

    <SettingSection title-key="settings.general.accent_color">
      <div class="accent">
        <button
          v-for="color in swatches"
          :key="color"
          type="button"
          class="accent__swatch"
          :style="{ background: color }"
          :aria-label="color"
          :aria-pressed="normalize(themeState.accent) === color"
          @click="applyAccent(color)"
        >
          <Check
            v-if="normalize(themeState.accent) === color"
            :size="13"
            :style="{ color: swatchInk(color) }"
          />
        </button>
        <label class="accent__custom" :style="{ background: themeState.accent }" :title="t('settings.appearance.accent_custom')">
          <Palette :size="13" />
          <input
            type="color"
            class="accent__input"
            :value="themeState.accent"
            :aria-label="t('settings.appearance.accent_custom')"
            @input="pickAccent"
          />
        </label>
      </div>
      <p class="appearance-tab__hint">{{ t("settings.appearance.accent_hint") }}</p>
    </SettingSection>

    <SettingSection title-key="settings.appearance.motion">
      <div class="motion">
        <CoSegmented v-model="motion" :options="motionOptions" />
      </div>
      <p class="appearance-tab__hint">{{ t("settings.appearance.motion_hint") }}</p>
    </SettingSection>
  </div>
</template>

<style scoped>
.appearance-tab {
  max-width: 760px;
}

.theme-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) 0 var(--copper-space-2);
}

.theme-card {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding: var(--copper-space-2);
  background: var(--copper-surface-raised, var(--copper-surface));
  border: 0;
  border-radius: var(--copper-radius-card);
  box-shadow: var(--copper-shadow-card);
  cursor: pointer;
  text-align: left;
  transition:
    transform var(--copper-motion-component-enter) ease,
    box-shadow var(--copper-motion-component-enter) ease,
    background-color var(--copper-motion-component-enter) ease;
}

.theme-card:hover {
  transform: translateY(-2px);
}

.theme-card--active {
  background: color-mix(in srgb, var(--copper-accent) 10%, var(--copper-surface-raised, var(--copper-surface)));
  box-shadow:
    0 14px 28px -20px color-mix(in srgb, var(--copper-accent) 65%, transparent),
    var(--copper-shadow-card);
  transform: translateY(-2px);
}

.theme-card__preview {
  position: relative;
  display: flex;
  height: 86px;
  gap: 2px;
  padding: 8px;
  overflow: hidden;
  border-radius: calc(var(--copper-radius-card) - 6px);
  background: #101418;
}

.preview-pane {
  position: relative;
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 4px;
  padding: 6px;
  border-radius: 6px;
}

.preview-pane--dark {
  background: #171c22;
}

.preview-pane--light {
  background: #f2f3f5;
}

.preview-pane__rail {
  width: 5px;
  height: 100%;
  position: absolute;
  top: 0;
  left: 0;
  border-radius: 6px 0 0 6px;
  background: color-mix(in srgb, currentColor 12%, transparent);
}

.preview-pane--dark {
  color: #ffffff;
}

.preview-pane--light {
  color: #101418;
}

.preview-pane > span {
  display: block;
  margin-left: 6px;
  border-radius: 3px;
  background: color-mix(in srgb, currentColor 16%, transparent);
}

.preview-pane__title {
  width: 46%;
  height: 6px;
  margin-top: 2px;
}

.preview-pane__line {
  width: 82%;
  height: 4px;
}

.preview-pane__line--short {
  width: 58%;
}

.preview-pane__chip {
  width: 30%;
  height: 8px;
  margin-top: auto;
  background: var(--copper-accent) !important;
  opacity: 0.85;
}

.theme-card__badge {
  position: absolute;
  top: 6px;
  right: 6px;
  display: grid;
  place-items: center;
  width: 18px;
  height: 18px;
  border-radius: 6px;
  color: #e8eaed;
  background: color-mix(in srgb, #101418 55%, transparent);
}

.theme-card__check {
  position: absolute;
  right: 6px;
  bottom: 6px;
  display: grid;
  place-items: center;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  color: var(--copper-accent-foreground);
  background: var(--copper-accent);
}

.theme-card__label {
  padding: 0 2px 2px;
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.theme-card--active .theme-card__label {
  color: var(--copper-text);
  font-weight: 600;
}

.appearance-tab__hint {
  margin: var(--copper-space-2) 0 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.motion {
  padding: var(--copper-space-3) 0 var(--copper-space-1);
}

.accent {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) 0 var(--copper-space-2);
}

.accent__swatch,
.accent__custom {
  position: relative;
  display: grid;
  place-items: center;
  width: 28px;
  height: 28px;
  padding: 0;
  border: 0;
  border-radius: 50%;
  cursor: pointer;
  box-shadow: 0 2px 6px -2px color-mix(in srgb, #000 32%, transparent);
  transition: transform var(--copper-motion-component-press) ease;
}

.accent__swatch:hover,
.accent__custom:hover {
  transform: scale(1.08);
}

.accent__check {
  color: #ffffff;
}

.accent__custom {
  color: #ffffff;
  background: var(--copper-accent);
}

.accent__input {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  padding: 0;
  border: 0;
  opacity: 0;
  cursor: pointer;
}

@media (max-width: 560px) {
  .theme-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
</style>
