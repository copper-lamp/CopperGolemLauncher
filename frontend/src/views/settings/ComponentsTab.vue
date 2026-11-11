<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { LoaderCircle } from '@lucide/vue';
import SettingSection from './SettingSection.vue';
import SettingRow from './SettingRow.vue';
import CoButton from '../../components/ui/CoButton.vue';
import CoSwitch from '../../components/ui/CoSwitch.vue';
import { useSettings } from '../../composables/useSettings';
import { useI18n } from '../../i18n';
import { componentsList, componentsCheck, componentsInstall, componentsCancel, type ComponentId, type ComponentStatus } from '../../api/components';
const { t }=useI18n(); const { get,set }=useSettings();
const rows=ref<ComponentStatus[]>([]); const busy=ref(false); const error=ref(''); let stop:UnlistenFn|undefined;
const automatic=computed({get:()=>get<boolean>('components.auto_update_enabled',true),set:(v:boolean)=>{void set('components.auto_update_enabled',v);}});
async function refresh(){rows.value=await componentsList();}
async function run(id:ComponentId,action:'check'|'install') {if(busy.value)return;busy.value=true;error.value='';try{if(action==='check')await componentsCheck(id);else await componentsInstall(id);}catch(e){error.value=String(e);}finally{await refresh();busy.value=false;}}
onMounted(async()=>{stop=await listen<ComponentStatus>('components-changed',()=>{void refresh();});await refresh();});
onUnmounted(()=>stop?.());
</script>
<template>
  <SettingSection title-key="components.title">
    <p class="components-note">{{t('components.description')}}</p>
    <SettingRow label-key="components.automatic" hint-key="components.automaticDescription"><CoSwitch v-model="automatic" /></SettingRow>
    <SettingRow v-for="row in rows" :key="row.id" :label-key="`components.names.${row.id}`">
      <span>{{row.version ?? t('components.notInstalled')}} · {{t(`components.states.${row.state}`)}}<template v-if="row.availableVersion"> · {{row.availableVersion}}</template> <template v-if="row.stage">· {{t(`components.stages.${row.stage}`)}}</template></span>
      <LoaderCircle v-if="row.state==='installing'" :size="16" class="components-spin" />
      <CoButton v-if="row.state==='installing'" @click="componentsCancel()">{{t('components.cancel')}}</CoButton>
      <CoButton :disabled="busy || !row.supported || row.state==='installing'" @click="run(row.id,'check')">{{t('components.check')}}</CoButton>
      <CoButton :disabled="busy || !row.supported || row.state==='installing'" @click="run(row.id,'install')">{{t(row.version ? 'components.update' : 'components.install')}}</CoButton>
    </SettingRow>
    <p v-if="error" role="alert">{{error}}</p>
    <p class="components-note">{{t('components.compatibility')}}</p>
  </SettingSection>
</template>
<style scoped>
.components-note{color:var(--copper-text-secondary);font-size:var(--copper-font-size-sm);line-height:1.6;}
.components-spin{flex-shrink:0;animation:components-spin 1s linear infinite;}html[data-motion="reduced"] .components-spin{animation:none;}@keyframes components-spin{to{transform:rotate(360deg);}}@media(prefers-reduced-motion:reduce){.components-spin{animation:none;}}
</style>
