<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { LoaderCircle } from '@lucide/vue';
import SettingSection from './SettingSection.vue';
import SettingRow from './SettingRow.vue';
import CoSwitch from '../../components/ui/CoSwitch.vue';
import { useSettings } from '../../composables/useSettings';
import { useI18n } from '../../i18n';
import { componentsList, type ComponentStatus } from '../../api/components';
const { t }=useI18n(); const { get,set }=useSettings();
const lip=ref<ComponentStatus|null>(null); let stop:UnlistenFn|undefined;
const automatic=computed({get:()=>get<boolean>('components.auto_update_enabled',true),set:(v:boolean)=>{void set('components.auto_update_enabled',v);}});
async function refresh(){lip.value=(await componentsList()).find(row=>row.id==='lip')??null;}
onMounted(async()=>{stop=await listen<ComponentStatus>('components-changed',event=>{if(event.payload.id==='lip')lip.value=event.payload;});await refresh();});
onUnmounted(()=>stop?.());
</script>
<template>
  <SettingSection title-key="components.title">
    <SettingRow label-key="components.lipStatus">
      <span v-if="lip" class="components-status">{{lip.version ?? t('components.notInstalled')}} · {{t(`components.states.${lip.state}`)}}<template v-if="lip.availableVersion"> · {{lip.availableVersion}}</template><template v-if="lip.stage"> · {{t(`components.stages.${lip.stage}`)}}</template></span>
      <LoaderCircle v-if="lip?.state==='installing'" :size="16" class="components-spin" />
    </SettingRow>
    <SettingRow label-key="components.automatic" hint-key="components.automaticDescription"><CoSwitch v-model="automatic" /></SettingRow>
  </SettingSection>
</template>
<style scoped>
.components-spin{flex-shrink:0;animation:components-spin 1s linear infinite;}html[data-motion="reduced"] .components-spin{animation:none;}@keyframes components-spin{to{transform:rotate(360deg);}}@media(prefers-reduced-motion:reduce){.components-spin{animation:none;}}
</style>
