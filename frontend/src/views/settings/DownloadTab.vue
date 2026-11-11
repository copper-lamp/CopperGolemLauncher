<script setup lang="ts">
import { computed } from 'vue';
import SettingSection from './SettingSection.vue';
import SettingRow from './SettingRow.vue';
import CoSelect from '../../components/ui/CoSelect.vue';
import CoSwitch from '../../components/ui/CoSwitch.vue';
import { useSettings } from '../../composables/useSettings';
import { useI18n } from '../../i18n';
const {get,set}=useSettings(); const {t}=useI18n();
const badge=computed({get:()=>get<boolean>('download.completion_badge_enabled',true),set:(v:boolean)=>void set('download.completion_badge_enabled',v)});
const history=computed({get:()=>String(get<number>('download.history_limit',50)),set:(v:string)=>void set('download.history_limit',Number(v))});
const retention=computed({get:()=>get<string>('game.installer_retention','never'),set:(v:string)=>void set('game.installer_retention',v)});
const historyOptions=computed(()=>[0,20,50,70,100,150,200].map(n=>({value:String(n),label:t(n===0?'settings.general.download_history_none':`settings.general.download_history_${n}`)})));
const retentionOptions=computed(()=>['after_install','1d','3d','7d','30d','never'].map(v=>({value:v,label:t(`settings.version.retention_${v}`)})));
</script>
<template><SettingSection title-key="settings.general.download_history"><SettingRow label-key="settings.general.completion_badge" hint-key="settings.general.completion_badge_hint"><CoSwitch v-model="badge"/></SettingRow><SettingRow label-key="settings.general.download_history" hint-key="settings.general.download_history_hint"><CoSelect v-model="history" :options="historyOptions"/></SettingRow><SettingRow label-key="settings.version.installer_retention"><CoSelect v-model="retention" :options="retentionOptions"/></SettingRow></SettingSection></template>
