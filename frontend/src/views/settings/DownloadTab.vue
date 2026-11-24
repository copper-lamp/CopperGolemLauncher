<script setup lang="ts">
import { computed } from "vue";
import { FolderOpen, RotateCcw } from "@lucide/vue";
import { open } from "@tauri-apps/plugin-dialog";
import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import CoSwitch from "../../components/ui/CoSwitch.vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";
const { get, set } = useSettings();
const { t } = useI18n();
const badge = computed({ get: () => get<boolean>("download.completion_badge_enabled", true), set: (v: boolean) => void set("download.completion_badge_enabled", v) });
const history = computed({ get: () => String(get<number>("download.history_limit", 50)), set: (v: string) => void set("download.history_limit", Number(v)) });
const retention = computed({ get: () => get<string>("game.installer_retention", "never"), set: (v: string) => void set("game.installer_retention", v) });
const fallbackDir = computed({ get: () => get<string>("download.fallback_dir", ""), set: (v: string) => void set("download.fallback_dir", v) });
const historyOptions = computed(() => [0, 20, 50, 70, 100, 150, 200].map(n => ({ value: String(n), label: t(n === 0 ? "settings.general.download_history_none" : `settings.general.download_history_${n}`) })));
const retentionOptions = computed(() => ["after_install", "1d", "3d", "7d", "30d", "never"].map(v => ({ value: v, label: t(`settings.version.retention_${v}`) })));
async function pickFallbackDir() { const selected = await open({ directory: true, multiple: false }); if (typeof selected === "string" && selected) fallbackDir.value = selected; }
function resetFallbackDir() { fallbackDir.value = ""; }
</script>
<template>
  <SettingSection title-key="settings.general.download_history">
    <SettingRow label-key="settings.general.completion_badge" hint-key="settings.general.completion_badge_hint"><CoSwitch v-model="badge" /></SettingRow>
    <SettingRow label-key="settings.general.download_history" hint-key="settings.general.download_history_hint"><CoSelect v-model="history" :options="historyOptions" /></SettingRow>
    <SettingRow label-key="settings.version.installer_retention"><CoSelect v-model="retention" :options="retentionOptions" /></SettingRow>
    <SettingRow label-key="settings.general.download_fallback_dir" hint-key="settings.general.download_fallback_dir_hint">
      <div class="download-tab__path"><CoTextField :model-value="fallbackDir" :placeholder="t('settings.general.download_fallback_dir_default')" @update:model-value="fallbackDir = $event" /><CoButton variant="secondary" size="sm" @click="pickFallbackDir"><FolderOpen :size="14" /><span>{{ t('settings.general.browse') }}</span></CoButton><CoButton variant="ghost" size="sm" @click="resetFallbackDir"><RotateCcw :size="14" /><span>{{ t('settings.general.reset') }}</span></CoButton></div>
    </SettingRow>
  </SettingSection>
</template>
<style scoped>.download-tab__path { display:flex; align-items:center; gap:var(--copper-space-2); min-width:min(560px, 55vw); } .download-tab__path :deep(.co-text-field) { min-width:0; flex:1; }</style>
