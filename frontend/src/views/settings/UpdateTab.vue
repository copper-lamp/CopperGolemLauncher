<script setup lang="ts">
import { computed } from 'vue';
import CoSwitch from '../../components/ui/CoSwitch.vue';
import CoSelect from '../../components/ui/CoSelect.vue';
import CoButton from '../../components/ui/CoButton.vue';
import SettingSection from './SettingSection.vue';
import SettingRow from './SettingRow.vue';
import {useSettings} from '../../composables/useSettings';
import {useUpdate} from '../../composables/useUpdate';
import {useI18n} from '../../i18n';
import {useSettingsText} from '../../services/settings-categories';
const {get,set}=useSettings(); const {t}=useI18n();const st=useSettingsText();
const {check,download,openPanel,busy,failed,error}=useUpdate();
const auto=computed({get:()=>get<boolean>('update.auto_check',true),set:(v:boolean)=>void set('update.auto_check',v)});
const channel=computed({get:()=>get<string>('update.channel','beta'),set:(v:string)=>void set('update.channel',v)});
const options=computed(()=>['stable','beta'].map(v=>({value:v,label:t(`settings.about.update_channel_${v}`)})));
</script>
<template><SettingSection title-key="settings.about.update_prefs"><SettingRow label-key="settings.about.auto_check" hint-key="settings.about.auto_check_hint"><CoSwitch v-model="auto"/></SettingRow><SettingRow label-key="settings.about.update_channel"><CoSelect v-model="channel" :options="options"/></SettingRow><div class="actions"><CoButton :disabled="busy" @click="check">{{t('update.check_now')}}</CoButton><CoButton :disabled="busy" @click="openPanel">{{st('update')}}</CoButton><CoButton v-if="failed" :disabled="busy" @click="download">{{st('retry')}}</CoButton></div><p v-if="error" role="alert">{{error}}</p></SettingSection></template>
<style scoped>.actions{display:flex;gap:var(--copper-space-3);flex-wrap:wrap}p{color:var(--copper-danger)}</style>
