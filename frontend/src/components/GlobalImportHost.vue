<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { useRoute } from 'vue-router';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { PackageOpen, ShieldAlert, Upload, X } from '@lucide/vue';
import { importsCancel, importsCommit, importsDrop, importsPending } from '../api/imports';
import { homeVersionsList, type VersionView } from '../api/home';
import { isKernelApiError } from '../api/core';
import { useSettings } from '../composables/useSettings';
import CoSelect from './ui/CoSelect.vue';
import { useKernelReady } from '../composables/useKernelReady';
import { showToast } from '../composables/useToast';
import { enqueueImports, importQueue, retireImport } from '../services/import-queue';
import { useImportText } from '../services/import-messages';
import { compareImportVersions } from '../services/import-version';
import { acquireModal, activeModalId, isModalActive } from '../services/modalCoordinator';
const text = useImportText();
let releaseModal: (() => void) | undefined;
const route = useRoute();
const settings = useSettings();
const { isKernelReady } = useKernelReady();
const current = computed(() => importQueue.value[0] ?? null);
const panel = ref<HTMLElement | null>(null);
watch(() => !!current.value, value => { if (value) releaseModal = acquireModal('global-import', 10); else { releaseModal?.(); releaseModal = undefined; } }, { immediate: true });
watch(activeModalId, async id => { if (id === 'global-import') { await nextTick(); panel.value?.focus(); } });
const instances = ref<VersionView[]>([]);
const instance = ref('');
const loading = ref(false);
const busy = ref(false);
const replace = ref(false);
const acknowledge = ref(false);
const conflict = ref(false);
const errorText = ref('');
const dragging = ref(false);
const modulePackage = computed(() => current.value?.kind === 'module');
const versionChange = computed(() => {
  const value = current.value;
  if (!value?.installedVersion || !value.version) return text('versionUnknown');
  const comparison = compareImportVersions(value.version, value.installedVersion);
  return text(comparison === null ? 'versionUnknown' : comparison > 0 ? 'upgrade' : comparison < 0 ? 'downgrade' : 'reinstall');
});
const replacementRequired = computed(() => modulePackage.value ? !!current.value?.installedVersion : conflict.value);
const allowed = computed(() => !!current.value && !busy.value && !loading.value && (modulePackage.value || instances.value.some(item => item.name === instance.value)) && (!replacementRequired.value || replace.value) && (!modulePackage.value || current.value?.publisherVerified || acknowledge.value));
let previousFocus: HTMLElement | null = null;
let disposed = false;
let pollBusy = false;
let timer: ReturnType<typeof setInterval> | undefined;
let offDrop: (() => void) | undefined;
async function loadInstances() {
  const ticket = current.value?.ticket;
  loading.value = true;
  try {
    const list = await homeVersionsList();
    if (current.value?.ticket !== ticket) return;
    instances.value = list;
    const queryName = typeof route.query.name === 'string' ? route.query.name : '';
    const preferred = list.some(item => item.name === queryName) ? queryName : settings.get<string>('launch.default_version', '');
    instance.value = list.some(item => item.name === preferred) ? preferred : '';
  } catch (error) { if (current.value?.ticket === ticket) errorText.value = String(error); }
  finally { if (current.value?.ticket === ticket) loading.value = false; }
}
watch(() => current.value?.ticket, async (ticket, oldTicket) => {
  replace.value = false; acknowledge.value = false; conflict.value = false; errorText.value = ''; instance.value = ''; loading.value = false;
  if (!ticket) { if (!activeModalId.value) previousFocus?.focus(); return; }
  if (!oldTicket) previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  await nextTick(); if (isModalActive('global-import')) panel.value?.focus();
  if (!modulePackage.value) await loadInstances();
});
watch(instance, () => { conflict.value = false; replace.value = false; errorText.value = ''; });
async function cancel() {
  if (!current.value || busy.value) return;
  busy.value = true;
  try { const ticket = current.value.ticket; await importsCancel(ticket); retireImport(ticket); }
  catch (error) { errorText.value = String(error); }
  finally { busy.value = false; }
}
async function commit() {
  if (!current.value || !allowed.value) return;
  busy.value = true; errorText.value = '';
  try {
    const ticket = current.value.ticket;
    await importsCommit(ticket, modulePackage.value ? null : instance.value, replace.value, acknowledge.value);
    retireImport(ticket); showToast(text('success'), 'success');
  } catch (error) {
    errorText.value = String(error);
    if (isKernelApiError(error) && error.kind === 'conflict') { conflict.value = true; replace.value = false; }
  } finally { busy.value = false; }
}
function keydown(event: KeyboardEvent) {
  if (!isModalActive('global-import')) return;
  if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); void cancel(); }
  if (event.key !== 'Tab') return;
  const nodes = panel.value?.querySelectorAll<HTMLElement>('button:not(:disabled),select:not(:disabled),input:not(:disabled),summary,[tabindex="0"]');
  if (!nodes?.length) { event.preventDefault(); return; }
  const first = nodes[0]!; const last = nodes[nodes.length - 1]!;
  if (event.shiftKey && (document.activeElement === first || document.activeElement === panel.value)) { event.preventDefault(); last.focus(); }
  else if (!event.shiftKey && (document.activeElement === last || document.activeElement === panel.value)) { event.preventDefault(); first.focus(); }
}
async function poll() {
  if (!isKernelReady.value || pollBusy || busy.value || importPickingActive()) return;
  pollBusy = true;
  try { const pending = await importsPending(); if (!disposed) enqueueImports(pending); }
  catch (error) {
    // Source inspection/copy failures need visible feedback; bridge outages stay in diagnostics.
    if (isKernelApiError(error) && !['ipc-unavailable', 'timeout'].includes(error.kind)) showToast(String(error), 'error');
  }
  finally { pollBusy = false; }
}
function importPickingActive() { return document.visibilityState === 'hidden'; }
const focus = () => { void poll(); };
onMounted(async () => {
  timer = setInterval(() => { void poll(); }, 1000);
  window.addEventListener('focus', focus); void poll();
  try {
    const off = await getCurrentWebview().onDragDropEvent(async ({ payload }) => {
      if (payload.type === 'over' || payload.type === 'enter') dragging.value = true;
      else dragging.value = false;
      if (payload.type === 'drop') {
        try { enqueueImports(await importsDrop(payload.paths)); }
        catch (error) { showToast(String(error), 'error'); }
      }
    });
    if (disposed) off(); else offDrop = off;
  } catch { /* Native file drops are unavailable in a plain browser. */ }
});
onUnmounted(() => { releaseModal?.(); disposed = true; if (timer) clearInterval(timer); offDrop?.(); window.removeEventListener('focus', focus); });
</script>
<template>
  <Teleport to="body">
    <div v-if="dragging" class="import-drop" role="status"><Upload :size="32" />{{ text('drop') }}</div>
    <div v-if="current && isModalActive('global-import')" class="import-mask" @keydown="keydown">
      <section ref="panel" class="import-panel" role="dialog" aria-modal="true" aria-labelledby="global-import-title" tabindex="-1" :aria-busy="busy">
        <header><PackageOpen :size="24" /><h2 id="global-import-title">{{ text('title') }}</h2><button :disabled="busy" :aria-label="text('cancel')" @click="cancel"><X :size="18" /></button></header>
        <p class="import-kind">{{ text(current.kind) }}</p><h3>{{ current.name }}</h3>
        <dl><dt>{{ text('filename') }}</dt><dd>{{ current.filename }}</dd><template v-if="current.version"><dt>{{ text('version') }}</dt><dd>{{ current.version }}</dd></template><template v-if="current.installedVersion"><dt>{{ text('installed') }}</dt><dd>{{ current.installedVersion }}</dd></template><template v-if="current.moduleId"><dt>ID</dt><dd>{{ current.moduleId }}</dd></template></dl>
        <template v-if="modulePackage">
          <h4>{{ text('permissions') }}</h4><ul v-if="current.permissions.length"><li v-for="permission in current.permissions" :key="permission">{{ permission }}</li></ul><p v-else>{{ text('noPermissions') }}</p>
          <div v-if="!current.publisherVerified" class="import-warning"><strong><ShieldAlert :size="18" />{{ text('unsigned') }}</strong><p>{{ text('risk') }}</p><label><input v-model="acknowledge" type="checkbox" :disabled="busy" />{{ text('acknowledge') }}</label></div>
          <details v-if="current.sha256"><summary>{{ text('hash') }}</summary><code>{{ current.sha256 }}</code></details>
        </template>
        <template v-else>
          <CoSelect v-model="instance" :label="text('instance')" :placeholder="text('choose')" :options="instances.map(item => ({ value: item.name, label: `${item.name} — ${item.game_version}` }))" :disabled="busy || loading" />
          <p v-if="loading" role="status">{{ text('loading') }}</p><p v-else-if="!instances.length">{{ text('noInstances') }} <button :disabled="busy" @click="loadInstances">{{ text('retryInstances') }}</button></p>
          <p v-if="current.kind === 'world'">{{ text('worldCopy') }}</p>
        </template>
        <div v-if="replacementRequired" class="import-warning"><strong v-if="modulePackage">{{ versionChange }}</strong><p>{{ text(modulePackage ? 'replacement' : 'conflict') }}</p><label><input v-model="replace" type="checkbox" :disabled="busy" />{{ text('replace') }}</label></div>
        <div v-if="errorText" class="import-error" role="alert"><p>{{ text('failed') }}</p><p>{{ errorText }}</p></div>
        <footer><span v-if="importQueue.length > 1">{{ text('next') }}: {{ importQueue.length - 1 }}</span><button :disabled="busy" @click="cancel">{{ text('cancel') }}</button><button class="import-primary" :disabled="!allowed" @click="commit"><Upload :size="16" />{{ text(busy ? 'working' : 'commit') }}</button></footer>
      </section>
    </div>
  </Teleport>
</template>
<style scoped>
.import-mask,.import-drop { position:fixed; inset:0; z-index:1100; display:grid; place-items:center; padding:20px; background:rgb(0 0 0 / .3); backdrop-filter:blur(9px); }
.import-drop { z-index:1090; pointer-events:none; align-content:center; justify-items:center; gap:16px; color:var(--copper-text); font-size:18px; }
.import-panel { box-sizing:border-box; width:min(600px,100%); max-height:calc(100dvh - 40px); overflow:auto; padding:24px; background:var(--copper-surface); color:var(--copper-text); border-radius:var(--copper-radius-card,16px); box-shadow:var(--copper-shadow-card); outline:none; }
header,footer { display:flex; align-items:center; gap:12px; } h2 { flex:1; margin:0; font-size:19px; } h3 { font-size:22px; margin:8px 0 18px; overflow-wrap:anywhere; } h4 { margin-bottom:8px; } p { line-height:1.6; overflow-wrap:anywhere; } .import-kind,dt { color:var(--copper-text-secondary); font-size:12px; }
dl { display:grid; grid-template-columns:120px minmax(0,1fr); gap:10px; } dd { margin:0; overflow-wrap:anywhere; } label { display:flex; align-items:flex-start; gap:9px; line-height:1.6; } input { margin-top:5px; accent-color:var(--copper-accent); flex-shrink:0; } select { width:100%; box-sizing:border-box; margin-top:8px; background:var(--copper-bg); color:inherit; border:0; padding:11px; border-radius:8px; }
.import-warning,.import-error { margin-top:16px; padding:14px; background:color-mix(in srgb,var(--copper-accent) 9%,var(--copper-surface)); border-radius:10px; } .import-warning strong { display:flex; gap:8px; align-items:center; } .import-error { background:color-mix(in srgb,var(--copper-danger) 9%,var(--copper-surface)); } code { display:block; overflow-wrap:anywhere; margin-top:10px; font-size:11px; } details { margin-top:16px; }
footer { margin-top:22px; justify-content:flex-end; flex-wrap:wrap; } footer span { flex:1; color:var(--copper-text-secondary); font-size:12px; } button { display:inline-flex; align-items:center; justify-content:center; gap:8px; cursor:pointer; border:0; border-radius:var(--copper-radius-control); padding:10px 14px; color:inherit; background:color-mix(in srgb,var(--copper-text) 7%,var(--copper-surface)); } button:hover { background:color-mix(in srgb,var(--copper-accent) 16%,var(--copper-surface)); } button:disabled { opacity:.45; cursor:default; } .import-primary { background:var(--copper-accent); color:var(--copper-on-accent,#fff); } :is(button,input,select,summary):focus-visible { outline:2px solid var(--copper-accent); outline-offset:3px; }
@media(max-width:600px) { .import-mask { padding:10px; } .import-panel { padding:18px; max-height:calc(100dvh - 20px); } dl { grid-template-columns:1fr; gap:5px; } dd { margin-bottom:8px; } }
@media(prefers-reduced-motion:reduce) { .import-mask,.import-drop { backdrop-filter:none; } }
</style>
