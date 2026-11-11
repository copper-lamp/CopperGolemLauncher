<script setup lang="ts">
import { computed, ref } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import { FolderOpen, Link } from '@lucide/vue';
import { homeCompatBind, homeCompatDiscover, homeCompatProbe, type CompatibilityProbe, type VersionView } from '../../api/home';
import { showToast } from '../../composables/useToast';
import { compatibilityReason, useCompatibilityText } from '../../services/compatibility-messages';
import CoButton from '../../components/ui/CoButton.vue';
const emit = defineEmits<{ bound: [version: VersionView] }>();
const text = useCompatibilityText();
const expanded = ref(false);
const source = ref('');
const alias = ref('');
const result = ref<CompatibilityProbe | null>(null);
const busy = ref(false);
const error = ref('');
const candidates = ref<string[]>([]);
async function discover() {
  if (busy.value) return;
  expanded.value = true; busy.value = true; error.value = '';
  try { candidates.value = await homeCompatDiscover(); }
  catch (e) { error.value = String(e); }
  finally { busy.value = false; }
}
async function chooseSource(path: string) {
  if (busy.value) return;
  source.value = path; result.value = null;
  const parts = path.split(/[\\/]/).filter(Boolean);
  alias.value = parts[parts.length - 1] ?? '';
  await probe();
}
const ready = computed(() => result.value?.supported && alias.value.trim() && !busy.value);
async function pick() {
  try {
    const path = await open({ directory: true, multiple: false });
    if (typeof path !== 'string') return;
    source.value = path; result.value = null;
    if (!alias.value) { const parts = path.split(/[\\/]/).filter(Boolean); alias.value = parts[parts.length - 1] ?? ''; }
    await probe();
  } catch (e) { error.value = String(e); }
}
async function probe() {
  if (!source.value || busy.value) return;
  busy.value = true; error.value = ''; result.value = null;
  try { result.value = await homeCompatProbe(source.value); }
  catch (e) { error.value = String(e); }
  finally { busy.value = false; }
}
async function bind() {
  if (!ready.value) return;
  busy.value = true; error.value = '';
  try {
    const version = await homeCompatBind(source.value, alias.value.trim());
    emit('bound', version); expanded.value = false; source.value = ''; result.value = null;
    showToast(text('success'), 'success');
  } catch (e) { error.value = String(e); }
  finally { busy.value = false; }
}
</script>
<template>
  <section class="binding">
    <CoButton v-if="!expanded" variant="secondary" @click="discover"><Link :size="16" />{{ text('bind') }}</CoButton>
    <form v-else class="binding__panel" :aria-busy="busy" @submit.prevent="bind">
      <strong>{{ text('title') }}</strong>
      <p>{{ text('sharedData') }}</p>
      <div v-if="candidates.length" class="binding__candidates"><span>{{ text('discovered') }}</span><CoButton v-for="candidate in candidates" :key="candidate" :disabled="busy" @click="chooseSource(candidate)">{{ candidate }}</CoButton></div>
      <label>{{ text('source') }}<input :value="source" readonly /></label>
      <div class="binding__actions"><CoButton variant="secondary" :disabled="busy" @click="pick"><FolderOpen :size="16" />{{ text('pick') }}</CoButton><CoButton v-if="source" variant="secondary" :disabled="busy" @click="probe">{{ text('probe') }}</CoButton></div>
      <label>{{ text('alias') }}<input v-model="alias" :disabled="busy" required maxlength="128" /></label>
      <p v-if="result">{{ result.game_version }} <span v-if="!result.supported">{{ compatibilityReason(result.disabled_reason, text) || text('unknown') }}</span></p>
      <p v-if="error" role="alert">{{ error }}</p>
      <div class="binding__actions"><CoButton type="submit" :disabled="!ready">{{ text('confirm') }}</CoButton><CoButton variant="secondary" :disabled="busy" @click="expanded = false">{{ text('cancel') }}</CoButton></div>
    </form>
  </section>
</template>
<style scoped>
.binding { min-width: 0; }
.binding__candidates { display: grid; gap: 6px; min-width: 0; max-height: 180px; overflow-y: auto; }
.binding__candidates :deep(button) { min-width: 0; white-space: normal; overflow-wrap: anywhere; height: auto; min-height: 36px; text-align: start; justify-content: start; }
.binding__panel { display: grid; gap: 12px; padding: 16px; max-width: 540px; border-radius: var(--radius-card, 16px); background: var(--copper-surface); box-shadow: var(--shadow-card, var(--copper-shadow)); }
p { margin: 0; color: var(--copper-text-secondary); overflow-wrap: anywhere; font-size: var(--copper-font-size-sm); }
label { display: grid; gap: 6px; font-size: var(--copper-font-size-sm); }
input { box-sizing: border-box; width: 100%; padding: 10px 12px; border: none; border-radius: var(--radius-control, 16px); background: var(--copper-bg); color: var(--copper-text); font: inherit; }
input:focus-visible { outline: 2px solid var(--copper-accent); outline-offset: 2px; }
.binding__actions { display: flex; flex-wrap: wrap; gap: 8px; }
</style>
