<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { ArrowLeft, ArrowRight, Check, LogOut, Package } from '@lucide/vue';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open } from '@tauri-apps/plugin-dialog';
import CompatibilityBinding from '../../modules/home/CompatibilityBinding.vue';
import CoSkeleton from '../ui/CoSkeleton.vue';
import { homeCompatDiscover, homeCompatProbe, homeCompatBind, type CompatibilityProbe } from '../../api/home';
import { storageStatus, storagePreview as previewStorage, storageApply, type StorageStatus, type StoragePreview, type StorageRequest } from '../../api/storage';
import { compatibilityReason, useCompatibilityText } from '../../services/compatibility-messages';
import { settingsGet, settingsSet } from '../../api/settings';
import type { JsonValue } from '../../api/types';
import { kernelInfo, themeSetMode, type ThemeMode } from '../../api/theme';
import { registryModules, registryStatus, pickLocalized, type RegistryModuleEntry } from '../../api/registry';
import { modulesInstall, modulesList } from '../../api/modules';
import { useI18n } from '../../i18n';
import { themeState } from '../../theme';

import { OnboardingSession, ONBOARDING_STEPS, RELEASE_KEY, parseReleaseState, planBriefs, acknowledgeBriefs, type ReleaseState } from '../../services/onboardingState';
import { RELEASE_BRIEFS } from '../../services/releaseBriefs';
import { onboardingKey as key } from '../../services/onboardingMessages';
import { acquireModal, guardModalFocus } from '../../services/modalCoordinator';
const { t, locale } = useI18n();
const session = new OnboardingSession({ read: settingsGet, write: (name, value) => settingsSet(name, value as JsonValue) });
const progress = ref(session.progress);
const loading = ref(true), visible = ref(true), busy = ref(false), error = ref(''), initialized = ref(false);
const panel = ref<HTMLElement | null>(null);
const modules = ref<RegistryModuleEntry[]>([]), moduleError = ref('');
const selected = ref<string[]>([]);
const briefs = ref<number[]>([]);
const releaseState = ref<ReleaseState | null>(null);
const platform = ref('');
const leviAvailable = ref(false);
const storage = ref<StorageStatus | null>(null), storageError = ref('');
const storagePreview = ref<StoragePreview | null>(null), storageScheduled = ref(false);
const leviCandidates = ref<CompatibilityProbe[]>([]), detectionError = ref('');
const compatibilityText = useCompatibilityText();
const boundSources = ref<string[]>([]);
const storageRequest = ref<StorageRequest | null>(null);
let releaseModal: (() => void) | undefined, releaseFocus: (() => void) | undefined;
const showingBriefs = computed(() => !loading.value && progress.value.completed && briefs.value.length > 0);
const step = computed(() => progress.value.step);
const steps = computed(() => ONBOARDING_STEPS.filter(s => s !== 'levi' || leviAvailable.value));
const briefItems = computed(() => briefs.value.flatMap(id => { const item = RELEASE_BRIEFS.find(x => x.id === id); return item ? [item] : []; }));
async function focusModal() {
  releaseModal?.(); releaseFocus?.();
  releaseModal = acquireModal(showingBriefs.value ? 'release-briefs' : 'onboarding', showingBriefs.value ? 50 : 100);
  await nextTick();
  releaseFocus = guardModalFocus(showingBriefs.value ? 'release-briefs' : 'onboarding', () => panel.value);
}
async function load() {
  error.value = ''; loading.value = true; visible.value = true; initialized.value = false;
  if (!releaseModal) await focusModal();
  try {
    await session.load(); progress.value = session.progress; selected.value = [...progress.value.selectedModules];
    const info = await kernelInfo(); platform.value = info.platform ?? '';
    if (!progress.value.completed) {
      await detectLevi();
      if (progress.value.step === 'levi' && !leviAvailable.value && !detectionError.value) { await session.save({ step: 'storage' }); progress.value = session.progress; }
    }
    const plan = planBriefs(info.version, parseReleaseState(await settingsGet(RELEASE_KEY)), RELEASE_BRIEFS);
    // Persist a pending batch before presenting it. A crash may repeat that batch, never lose it.
    await settingsSet(RELEASE_KEY, plan.state as unknown as JsonValue);
    releaseState.value = plan.state; briefs.value = plan.ids;
    loading.value = false; initialized.value = true;
    if (progress.value.completed && !briefs.value.length) close();
    else { await focusModal(); if (!progress.value.completed) void loadCapabilities(); }
  } catch (e) { error.value = `${t(key('saveFailed'))} ${String(e)}`; loading.value = false; }
}
async function loadCapabilities() {
  try { storage.value = await storageStatus(); storageScheduled.value = storage.value.restartRequired; } catch { storageError.value = t(key('storageError')); }
  try {
    const status = await registryStatus();
    if (!status.available || !status.signature_verified) return;
    const [entries, installed] = await Promise.all([registryModules(locale.value), modulesList()]);
    const installedIds = new Set(installed.map(x => x.id));
    modules.value = entries.filter(entry => entry.status === 'approved' && !entry.yanked && entry.platforms?.includes(platform.value) && !installedIds.has(entry.id));
  } catch { moduleError.value = t(key('noModules')); }
}
async function detectLevi() {
  detectionError.value = '';
  try {
    const candidates = await homeCompatDiscover();
    leviAvailable.value = candidates.length > 0;
    const results: CompatibilityProbe[] = [];
    // Bound concurrency: discovery may return thousands of local instances.
    for (let index = 0; index < candidates.length; index += 4) {
      const batch = await Promise.all(candidates.slice(index, index + 4).map(async source => {
        try { return await homeCompatProbe(source); }
        catch { return { supported: false, disabled_reason: 'source_missing', source_dir: source, game_version: null, version_type: null }; }
      }));
      results.push(...batch);
    }
    leviCandidates.value = results;
  }
  catch { detectionError.value = t(key('detectError')); }
}
async function bindCandidate(candidate: CompatibilityProbe) {
  if (!candidate.supported || busy.value) return;
  busy.value = true; error.value = '';
  try {
    const segments = candidate.source_dir.split(/[\\/]/).filter(Boolean);
    const name = segments[segments.length - 1] ?? '';
    if (!name || new TextEncoder().encode(name).length > 64) throw new Error(t(key('aliasRequired')));
    await homeCompatBind(candidate.source_dir, name);
    boundSources.value = [...boundSources.value, candidate.source_dir];
  } catch (e) { error.value = String(e); } finally { busy.value = false; }
}
async function chooseStorage(root: 'gameRoot' | 'launcherRoot') {
  busy.value = true; storageError.value = '';
  try {
    const path = await open({ directory: true, multiple: false });
    if (typeof path !== 'string') return;
    storageRequest.value = { [root]: path };
    storagePreview.value = await previewStorage(storageRequest.value);
  } catch (e) { storageError.value = String(e); } finally { busy.value = false; }
}
async function applyStorage() {
  if (!storageRequest.value || !storagePreview.value) return;
  busy.value = true; storageError.value = '';
  try {
    const outcome = await storageApply(storageRequest.value);
    storageScheduled.value = outcome.scheduled && outcome.restartRequired;
    storagePreview.value = null;
  } catch (e) { storageError.value = String(e); } finally { busy.value = false; }
}
async function saveSelection() {
  if (busy.value || !initialized.value) return;
  busy.value = true; moduleError.value = '';
  try { await session.save({ selectedModules: selected.value }); progress.value = session.progress; }
  catch (e) { selected.value = [...session.progress.selectedModules]; moduleError.value = `${t(key('saveFailed'))} ${String(e)}`; }
  finally { busy.value = false; }
}
async function next() {
  if (busy.value || !initialized.value) return; busy.value = true; error.value = '';
  try {
    await session.save({ selectedModules: selected.value });
    await session.advance(leviAvailable.value); progress.value = session.progress;
    if (progress.value.completed) { if (briefs.value.length) await focusModal(); else close(); }
    else { await nextTick(); panel.value?.focus(); }
  } catch (e) { error.value = `${t(key('saveFailed'))} ${String(e)}`; } finally { busy.value = false; }
}
async function back() {
  const previous = steps.value[steps.value.indexOf(step.value) - 1]; if (!previous || busy.value || !initialized.value) return;
  busy.value = true;
  try { await session.save({ step: previous, selectedModules: selected.value }); progress.value = session.progress; }
  catch (e) { error.value = `${t(key('saveFailed'))} ${String(e)}`; } finally { busy.value = false; }
}
async function changeTheme(mode: ThemeMode) {
  busy.value = true; error.value = '';
  try { await themeSetMode(mode); themeState.mode = mode; document.documentElement.dataset.themeMode = mode; document.documentElement.dataset.theme = mode === 'auto' ? (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light') : mode; }
  catch (e) { error.value = String(e); } finally { busy.value = false; }
}
async function installSelected() {
  busy.value = true; moduleError.value = '';
  try {
    await session.save({ selectedModules: selected.value });
    for (const id of selected.value) {
      if (session.progress.installedModules.includes(id)) continue;
      if (!modules.value.some(entry => entry.id === id)) continue;
      await modulesInstall(id);
      await session.save({ installedModules: [...session.progress.installedModules, id] }); progress.value = session.progress;
    }
  } catch (e) { moduleError.value = `${t(key('moduleFailed'))} ${String(e)}`; } finally { busy.value = false; }
}
async function dismissBriefs() {
  if (!releaseState.value || busy.value) return; busy.value = true; error.value = '';
  try { await settingsSet(RELEASE_KEY, acknowledgeBriefs(releaseState.value) as unknown as JsonValue); briefs.value = []; close(); }
  catch (e) { error.value = `${t(key('saveFailed'))} ${String(e)}`; } finally { busy.value = false; }
}
function close() { visible.value = false; releaseModal?.(); releaseModal = undefined; releaseFocus?.(); releaseFocus = undefined; }
async function exit() { try { await getCurrentWindow().close(); } catch (e) { error.value = String(e); } }
onMounted(load);
onBeforeUnmount(close);
</script>
<template>
  <Teleport to="body">
    <div v-if="visible" class="onboarding-mask">
      <section ref="panel" class="onboarding-panel" role="dialog" aria-modal="true" aria-labelledby="onboarding-title" tabindex="-1">
        <template v-if="loading"><h1 id="onboarding-title">{{ t(key('title')) }}</h1><CoSkeleton variant="detail" :label="t(key('loading'))" /></template>
        <template v-else-if="showingBriefs">
          <h1 id="onboarding-title">{{ t(key('briefs')) }}</h1>
          <article v-for="item in briefItems" :key="item.id" class="brief"><img :src="item.image" :alt="t(item.titleKey)" width="96" height="96"/><div><small>{{ item.version }}</small><h2>{{ t(item.titleKey) }}</h2><p>{{ t(item.bodyKey) }}</p></div></article>
          <footer><button :disabled="busy" class="primary" @click="dismissBriefs">{{ t(key('dismiss')) }}</button></footer>
        </template>
        <template v-else>
          <small class="step-count">{{ steps.indexOf(step) + 1 }} / {{ steps.length }}</small>
          <h1 id="onboarding-title">{{ t(key(step)) }}</h1><p>{{ t(key(`${step}.body`)) }}</p>
          <div v-if="step === 'theme'" class="theme-options"><button v-for="mode in (['light','dark','auto'] as const)" :key="mode" :aria-pressed="themeState.mode === mode" :disabled="busy" @click="changeTheme(mode)">{{ t(key(mode)) }}</button></div>
          <p v-if="detectionError && step === 'theme'" role="status">{{ detectionError }} <button :disabled="busy" @click="detectLevi">{{ t(key('retry')) }}</button></p>
          <template v-if="step === 'storage'"><dl v-if="storage"><dt>{{ t(key('launcherRoot')) }}</dt><dd>{{ storage.launcherRoot }}</dd><dt>{{ t(key('gameRoot')) }}</dt><dd>{{ storage.gameRoot }}</dd></dl><p v-if="storage?.lastError" role="alert">{{ storage.lastError }}</p><p v-if="storageError" role="status">{{ storageError }}</p><button :disabled="busy || !storage || storage.environmentOverride || storageScheduled" :title="storage?.environmentOverride ? t(key('unavailable')) : undefined" @click="chooseStorage('gameRoot')">{{ t(key('chooseStorage')) }}</button><button :disabled="busy || !storage || storage.environmentOverride || storageScheduled" :title="storage?.environmentOverride ? t(key('unavailable')) : undefined" @click="chooseStorage('launcherRoot')">{{ t(key('chooseLauncherStorage')) }}</button><div v-if="storagePreview"><p>{{ storagePreview.launcherRoot }}</p><p>{{ storagePreview.gameRoot }}</p><p>{{ t(key('confirmStorage')) }}</p><button :disabled="busy" @click="applyStorage">{{ t(key('confirm')) }}</button><button :disabled="busy" @click="storagePreview=null">{{ t(key('cancel')) }}</button></div><p v-if="storageScheduled" role="status">{{ t(key('storagePending')) }}</p></template>
          <template v-if="step === 'levi'"><div v-for="candidate in leviCandidates" :key="candidate.source_dir" class="module-row"><span><strong>{{ candidate.game_version }}</strong><small>{{ candidate.source_dir }}</small><small v-if="!candidate.supported">{{ compatibilityReason(candidate.disabled_reason,compatibilityText) || t(key('unavailable')) }}</small></span><button :disabled="busy || !candidate.supported || boundSources.includes(candidate.source_dir)" @click="bindCandidate(candidate)">{{ t(key(boundSources.includes(candidate.source_dir) ? 'bound' : 'bind')) }}</button></div><CompatibilityBinding /></template>
          <template v-if="step === 'modules'">
            <p v-if="!modules.length" role="status">{{ t(key('noModules')) }}</p>
            <label v-for="item in modules" :key="item.id" class="module-row"><input v-model="selected" type="checkbox" :value="item.id" :disabled="busy || progress.installedModules.includes(item.id)" @change="saveSelection"/><Package :size="20"/><span><strong>{{ item.display_name || item.id }}</strong><small>{{ item.author?.name }} · {{ item.version }}</small><small>{{ pickLocalized(item.summary,locale) }}</small><small>{{ t(key('permissions')) }}: {{ item.permissions?.join(', ') || '—' }}</small><small v-if="progress.installedModules.includes(item.id)">{{ t(key('installed')) }}</small></span></label>
            <p v-if="moduleError" role="alert">{{ moduleError }}</p><button v-if="modules.length" :disabled="busy || !selected.some(id => !progress.installedModules.includes(id))" @click="installSelected"><Check :size="16"/>{{ t(key(busy ? 'installing' : 'install')) }}</button>
          </template>
          <footer><button :disabled="busy" @click="exit"><LogOut :size="16"/>{{ t(key('exit')) }}</button><span/><button v-if="steps.indexOf(step)>0" :disabled="busy" @click="back"><ArrowLeft :size="16"/>{{ t(key('back')) }}</button><button :disabled="busy || !initialized || !!storagePreview" class="primary" @click="next">{{ t(key(step === 'modules' ? 'finish' : 'next')) }}<ArrowRight :size="16"/></button></footer>
        </template>
        <div v-if="error" role="alert" class="error"><p>{{ error }}</p><button :disabled="busy" @click="load">{{ t(key('retry')) }}</button></div>
      </section>
    </div>
  </Teleport>
</template>
<style scoped>
.onboarding-mask{position:fixed;inset:0;z-index:1600;display:grid;place-items:center;padding:24px;background:rgb(0 0 0 / .36);backdrop-filter:blur(12px);-webkit-backdrop-filter:blur(12px)}
.onboarding-panel{width:min(680px,100%);max-height:calc(100dvh - 48px);overflow:auto;box-sizing:border-box;padding:32px;border-radius:var(--radius-card,16px);background:var(--copper-surface);color:var(--copper-text);box-shadow:var(--shadow-card,var(--copper-shadow));outline:none}
h1{font-size:25px;margin:10px 0 18px}h2{font-size:18px}p{line-height:1.75;color:var(--copper-text-secondary)}.step-count,small{color:var(--copper-text-secondary)}footer{display:flex;align-items:center;gap:10px;margin-top:28px;flex-wrap:wrap}footer>span{flex:1}button{display:inline-flex;align-items:center;justify-content:center;gap:8px;padding:11px 16px;border:0;border-radius:var(--radius-control,16px);background:var(--copper-hover);color:inherit;cursor:pointer}button.primary,button[aria-pressed=true]{background:var(--copper-accent);color:var(--copper-accent-foreground)}button:disabled{opacity:.5;cursor:not-allowed}button:focus-visible,input:focus-visible{outline:2px solid var(--copper-accent);outline-offset:3px}.theme-options{display:flex;gap:12px;margin:24px 0}.module-row{display:flex;gap:12px;padding:14px;border-radius:var(--radius-card,16px);background:var(--copper-hover);margin-bottom:10px}.module-row span{display:grid;gap:5px}.brief{display:flex;gap:20px;margin-block:20px}.brief img{object-fit:contain;align-self:center}.brief h2{margin:5px 0}.error{padding:12px;border-radius:var(--radius-control,16px);background:var(--copper-hover)}dl{font-size:12px;overflow-wrap:anywhere}dt{font-weight:600}dd{margin:4px 0 12px}@media(max-width:520px){.onboarding-mask{padding:12px}.onboarding-panel{padding:22px;max-height:calc(100dvh - 24px)}.brief{flex-direction:column}.brief img{align-self:start}footer>span{display:none}}@media(prefers-reduced-motion:no-preference){.onboarding-panel{animation:arrive 180ms ease-out}@keyframes arrive{from{opacity:0;transform:translateY(8px) scale(.985)}to{opacity:1;transform:none}}}
</style>
