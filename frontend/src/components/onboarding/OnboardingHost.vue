<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { ArrowLeft, ArrowRight, Check, LogOut, Package, LayoutDashboard, Download, FolderOpen, Play, Palette, Settings, Boxes } from '@lucide/vue';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open } from '@tauri-apps/plugin-dialog';
import { call } from '../../api/core';
import { storageStatus, storagePreview as previewStorage, storageApply, type StorageStatus, type StoragePreview, type StorageRequest } from '../../api/storage';
import { settingsGet, settingsSet } from '../../api/settings';
import type { JsonValue } from '../../api/types';
import { kernelInfo, themeSetMode, themeSetAccent, type ThemeMode } from '../../api/theme';
import { registryModules, registryStatus, pickLocalized, type RegistryModuleEntry } from '../../api/registry';
import { modulesInstall, modulesList } from '../../api/modules';
import { useI18n } from '../../i18n';
import { themeState } from '../../theme';
import { OnboardingSession, ONBOARDING_STEPS, RELEASE_KEY, parseReleaseState, planBriefs, acknowledgeBriefs, type ReleaseState } from '../../services/onboardingState';
import { RELEASE_BRIEFS } from '../../services/releaseBriefs';
import { onboardingKey as key } from '../../services/onboardingMessages';
import { acquireModal, guardModalFocus } from '../../services/modalCoordinator';
const { t, locale } = useI18n();
const props = withDefaults(defineProps<{ preview?: boolean }>(), { preview: false });
const emit = defineEmits<{ closed: [] }>();
// Preview owns a disposable store; it never reads or writes the real lifecycle keys.
const previewMode = props.preview && import.meta.env.DEV;
const previewStore = new Map<string, unknown>();
const session = new OnboardingSession(previewMode
  ? { read: async name => previewStore.get(name) ?? null, write: async (name, value) => { previewStore.set(name, value); } }
  : { read: settingsGet, write: (name, value) => settingsSet(name, value as JsonValue) });
const progress = ref(session.progress), visible = ref(true), busy = ref(false), error = ref(''), initialized = ref(false);
const panel = ref<HTMLElement | null>(null), modules = ref<RegistryModuleEntry[]>([]), moduleError = ref('');
const selected = ref<string[]>([]), briefs = ref<number[]>([]), releaseState = ref<ReleaseState | null>(null), platform = ref('');
const storage = ref<StorageStatus | null>(null), storageError = ref(''), storagePreview = ref<StoragePreview | null>(null), storageScheduled = ref(false);
const storageRequest = ref<StorageRequest | null>(null);
interface LeviPreferences { installations: { configPath: string; versionsDirectory: string; colors: { light?: string; dark?: string } }[]; warnings: string[] }
const preferences = ref<LeviPreferences | null>(null), preferencesLoading = ref(false), configurationError = ref(''), appliedPreferences = ref<string[]>([]);
let releaseModal: (() => void) | undefined, releaseFocus: (() => void) | undefined;
let initialization: Promise<void> | undefined, disposed = false;
const showingBriefs = computed(() => initialized.value && progress.value.completed && briefs.value.length > 0);
const step = computed(() => progress.value.step);
const steps = ONBOARDING_STEPS;
const briefItems = computed(() => briefs.value.flatMap(id => { const item = RELEASE_BRIEFS.find(x => x.id === id); return item ? [item] : []; }));
const previewIcon = computed(() => ({ features: LayoutDashboard, theme: Palette, levi: FolderOpen, storage: Settings, game: Download, content: Boxes, launch: Play, modules: Package })[step.value]);
async function focusModal() {
  releaseModal?.(); releaseFocus?.();
  const id = previewMode ? 'onboarding-preview' : showingBriefs.value ? 'release-briefs' : 'onboarding';
  releaseModal = acquireModal(id, showingBriefs.value ? 50 : 100);
  await nextTick();
  if (!disposed) releaseFocus = guardModalFocus(id, () => panel.value);
}
function load() {
  if (initialization) return initialization;
  initialization = initialize().finally(() => { initialization = undefined; });
  return initialization;
}
async function initialize() {
  error.value = '';
  if (!releaseModal) await focusModal();
  if (previewMode) { initialized.value = true; return; }
  try {
    // Render the feature introduction immediately; saved state is authoritative once read.
    await session.load();
    if (disposed) return;
    progress.value = session.progress; selected.value = [...progress.value.selectedModules];
    const info = await kernelInfo(); platform.value = info.platform ?? '';
    const plan = planBriefs(info.version, parseReleaseState(await settingsGet(RELEASE_KEY)), RELEASE_BRIEFS);
    await settingsSet(RELEASE_KEY, plan.state as unknown as JsonValue);
    if (disposed) return;
    releaseState.value = plan.state; briefs.value = plan.ids; initialized.value = true;
    if (progress.value.completed && !briefs.value.length) close();
    else { await focusModal(); if (!progress.value.completed) { void loadCapabilities(); void readPreferences(); } }
  } catch (e) { if (!disposed) error.value = `${t(key('saveFailed'))} ${String(e)}`; }
}
async function loadCapabilities() {
  if (previewMode) return;
  try { storage.value = await storageStatus(); storageScheduled.value = storage.value.restartRequired; } catch { storageError.value = t(key('storageError')); }
  try {
    const status = await registryStatus();
    if (!status.available || !status.signature_verified) return;
    const [entries, installed] = await Promise.all([registryModules(locale.value), modulesList()]);
    const installedIds = new Set(installed.map(x => x.id));
    modules.value = entries.filter(x => x.status === 'approved' && !x.yanked && x.platforms?.includes(platform.value) && !installedIds.has(x.id));
  } catch { moduleError.value = t(key('noModules')); }
}
async function readPreferences() {
  if (previewMode || preferencesLoading.value) return;
  preferencesLoading.value = true; configurationError.value = '';
  try { preferences.value = await call<LeviPreferences>('levi_preferences_read'); }
  catch (e) { configurationError.value = String(e); }
  finally { preferencesLoading.value = false; }
}
async function applyPreferences(item: LeviPreferences['installations'][number]) {
  if (previewMode || busy.value) return;
  busy.value = true; configurationError.value = '';
  try {
    // The directory apply contract is coordinated with the storage owner.
    await call('levi_preferences_select_versions', { versionsDirectory: item.versionsDirectory });
    const dark = themeState.mode === 'dark' || (themeState.mode === 'auto' && matchMedia('(prefers-color-scheme: dark)').matches);
    const accent = dark ? item.colors.dark : item.colors.light;
    if (accent) {
      await themeSetAccent(accent); themeState.accent = accent;
      document.documentElement.style.setProperty('--copper-accent', accent);
      const hex = accent.replace('#', '');
      const brightness = .2126 * parseInt(hex.slice(0, 2), 16) + .7152 * parseInt(hex.slice(2, 4), 16) + .0722 * parseInt(hex.slice(4, 6), 16);
      document.documentElement.style.setProperty('--copper-accent-foreground', brightness > 140 ? '#0f1419' : '#ffffff');
    }
    appliedPreferences.value = [...appliedPreferences.value, item.configPath];
  } catch (e) { configurationError.value = String(e); }
  finally { busy.value = false; }
}
async function chooseStorage(root: 'gameRoot' | 'launcherRoot') {
  if (previewMode) return;
  busy.value = true; storageError.value = '';
  try { const path = await open({ directory: true, multiple: false }); if (typeof path !== 'string') return; storageRequest.value = { [root]: path }; storagePreview.value = await previewStorage(storageRequest.value); }
  catch (e) { storageError.value = String(e); } finally { busy.value = false; }
}
async function applyStorage() {
  if (previewMode || !storageRequest.value || !storagePreview.value) return;
  busy.value = true; storageError.value = '';
  try { const result = await storageApply(storageRequest.value); storageScheduled.value = result.scheduled && result.restartRequired; storagePreview.value = null; }
  catch (e) { storageError.value = String(e); } finally { busy.value = false; }
}
async function saveSelection() {
  if (busy.value || !initialized.value) return;
  busy.value = true; moduleError.value = '';
  try { await session.save({ selectedModules: selected.value }); progress.value = session.progress; }
  catch (e) { selected.value = [...session.progress.selectedModules]; moduleError.value = `${t(key('saveFailed'))} ${String(e)}`; } finally { busy.value = false; }
}
async function next() {
  if (busy.value || !initialized.value) return;
  busy.value = true; error.value = '';
  try { await session.save({ selectedModules: selected.value }); await session.advance(true); progress.value = session.progress; if (progress.value.completed) { if (briefs.value.length) await focusModal(); else close(); } else { await nextTick(); panel.value?.focus(); } }
  catch (e) { error.value = `${t(key('saveFailed'))} ${String(e)}`; } finally { busy.value = false; }
}
async function back() {
  const previous = steps[steps.indexOf(step.value) - 1]; if (!previous || busy.value || !initialized.value) return;
  busy.value = true;
  try { await session.save({ step: previous, selectedModules: selected.value }); progress.value = session.progress; }
  catch (e) { error.value = `${t(key('saveFailed'))} ${String(e)}`; } finally { busy.value = false; }
}
async function changeTheme(mode: ThemeMode) {
  if (previewMode) return;
  busy.value = true; error.value = '';
  try { await themeSetMode(mode); themeState.mode = mode; document.documentElement.dataset.themeMode = mode; document.documentElement.dataset.theme = mode === 'auto' ? (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light') : mode; }
  catch (e) { error.value = String(e); } finally { busy.value = false; }
}
async function installSelected() {
  if (previewMode) return;
  busy.value = true; moduleError.value = '';
  try { await session.save({ selectedModules: selected.value }); for (const id of selected.value) { if (session.progress.installedModules.includes(id) || !modules.value.some(x => x.id === id)) continue; await modulesInstall(id); await session.save({ installedModules: [...session.progress.installedModules, id] }); progress.value = session.progress; } }
  catch (e) { moduleError.value = `${t(key('moduleFailed'))} ${String(e)}`; } finally { busy.value = false; }
}
async function dismissBriefs() {
  if (previewMode || !releaseState.value || busy.value) return;
  busy.value = true; error.value = '';
  try { await settingsSet(RELEASE_KEY, acknowledgeBriefs(releaseState.value) as unknown as JsonValue); briefs.value = []; close(); }
  catch (e) { error.value = `${t(key('saveFailed'))} ${String(e)}`; } finally { busy.value = false; }
}
function close() { const wasVisible = visible.value; visible.value = false; releaseModal?.(); releaseModal = undefined; releaseFocus?.(); releaseFocus = undefined; if (previewMode && wasVisible) emit('closed'); }
async function exit() { if (previewMode) { close(); return; } try { await getCurrentWindow().close(); } catch (e) { error.value = String(e); } }
onMounted(load);
onBeforeUnmount(() => { disposed = true; close(); });
</script>
<template>
  <Teleport to="body">
    <div v-if="visible" class="onboarding-mask">
      <section ref="panel" class="onboarding-panel" role="dialog" aria-modal="true" aria-labelledby="onboarding-title" tabindex="-1">
        <template v-if="showingBriefs">
          <div class="copy"><h1 id="onboarding-title">{{ t(key('briefs')) }}</h1><article v-for="item in briefItems" :key="item.id" class="brief"><img :src="item.image" :alt="t(item.titleKey)" width="96" height="96"/><div><small>{{ item.version }}</small><h2>{{ t(item.titleKey) }}</h2><p>{{ t(item.bodyKey) }}</p></div></article></div>
          <p v-if="error" role="alert">{{ error }}</p><footer><span/><button :disabled="busy" class="primary" @click="dismissBriefs">{{ t(key('dismiss')) }}</button></footer>
        </template>
        <template v-else>
          <div class="launcher-preview" aria-hidden="true">
            <aside><img src="/app-64.png" alt="" width="32" height="32"/><LayoutDashboard/><Download/><Boxes/><Settings/></aside>
            <div class="preview-workspace"><div class="preview-toolbar"><span>{{ t(key('preview.brand')) }}</span><i/><i/><i/></div><div class="preview-body"><div class="preview-heading"><component :is="previewIcon" :size="30"/><strong>{{ t(key(step)) }}</strong></div><div class="preview-grid"><div class="preview-feature"><component :is="previewIcon" :size="54"/><span>{{ t(key(`${step}.preview`)) }}</span></div><div class="preview-list"><div v-for="n in 3" :key="n"><span/><i/></div><button tabindex="-1"><Play :size="18"/>{{ t(key('preview.launch')) }}</button></div></div></div></div>
          </div>
          <div class="copy"><p v-if="previewMode" role="status">{{ t(key('preview.notice')) }}</p><small>{{ t(key('title')) }}</small><h1 id="onboarding-title">{{ t(key(step)) }}</h1><p class="description">{{ t(key(`${step}.body`)) }}</p>
            <div v-if="step === 'theme'" class="controls"><button v-for="mode in (['light','dark','auto'] as const)" :key="mode" :aria-pressed="themeState.mode === mode" :disabled="previewMode || busy || !initialized" @click="changeTheme(mode)">{{ t(key(mode)) }}</button></div>
            <div v-if="step === 'levi'" class="details">
              <p v-if="preferencesLoading" role="status">{{ t(key('configLoading')) }}</p><p v-else-if="!preferences?.installations.length">{{ t(key('noConfiguration')) }}</p>
              <div v-for="item in preferences?.installations" :key="item.configPath" class="module-row"><span><strong>{{ item.versionsDirectory }}</strong><small>{{ item.configPath }}</small><small v-if="item.colors.light || item.colors.dark">{{ t(key('colors')) }}: {{ item.colors.light }} {{ item.colors.dark }}</small></span><button :disabled="busy || appliedPreferences.includes(item.configPath)" @click="applyPreferences(item)">{{ t(key(appliedPreferences.includes(item.configPath) ? 'applied' : 'applyConfiguration')) }}</button></div>
              <p v-for="warning in preferences?.warnings" :key="warning">{{ warning }}</p><p v-if="configurationError" role="alert">{{ configurationError }} <button :disabled="preferencesLoading" @click="readPreferences">{{ t(key('retry')) }}</button></p>
            </div>
            <div v-if="step === 'storage'" class="details"><dl v-if="storage"><dt>{{ t(key('launcherRoot')) }}</dt><dd>{{ storage.launcherRoot }}</dd><dt>{{ t(key('gameRoot')) }}</dt><dd>{{ storage.gameRoot }}</dd></dl><p v-if="storage?.lastError">{{ storage.lastError }}</p><p v-if="storageError">{{ storageError }}</p><div class="controls"><button :disabled="busy || !storage || storage.environmentOverride || storageScheduled" @click="chooseStorage('gameRoot')">{{ t(key('chooseStorage')) }}</button><button :disabled="busy || !storage || storage.environmentOverride || storageScheduled" @click="chooseStorage('launcherRoot')">{{ t(key('chooseLauncherStorage')) }}</button></div><div v-if="storagePreview"><p>{{ storagePreview.launcherRoot }}</p><p>{{ storagePreview.gameRoot }}</p><p>{{ t(key('confirmStorage')) }}</p><button :disabled="busy" @click="applyStorage">{{ t(key('confirm')) }}</button><button :disabled="busy" @click="storagePreview=null">{{ t(key('cancel')) }}</button></div><p v-if="storageScheduled" role="status">{{ t(key('storagePending')) }}</p></div>
            <div v-if="step === 'modules'" class="details"><p v-if="!modules.length">{{ t(key('noModules')) }}</p><label v-for="item in modules" :key="item.id" class="module-row"><input v-model="selected" type="checkbox" :value="item.id" :disabled="busy || progress.installedModules.includes(item.id)" @change="saveSelection"/><Package :size="20"/><span><strong>{{ item.display_name || item.id }}</strong><small>{{ item.author?.name }} · {{ item.version }}</small><small>{{ pickLocalized(item.summary,locale) }}</small><small>{{ t(key('permissions')) }}: {{ item.permissions?.join(', ') || '—' }}</small><small v-if="progress.installedModules.includes(item.id)">{{ t(key('installed')) }}</small></span></label><p v-if="moduleError" role="alert">{{ moduleError }}</p><button v-if="modules.length" :disabled="busy || !selected.some(id => !progress.installedModules.includes(id))" @click="installSelected"><Check :size="16"/>{{ t(key(busy ? 'installing' : 'install')) }}</button></div>
            <div v-if="error" role="alert" class="error"><p>{{ error }}</p><button :disabled="busy" @click="load">{{ t(key('retry')) }}</button></div>
          </div>
          <footer><button :disabled="busy" @click="exit"><LogOut :size="16"/>{{ t(key(previewMode ? 'preview.close' : 'exit')) }}</button><div class="progress" role="progressbar" :aria-label="t(key('progress'))" :aria-valuenow="steps.indexOf(step)+1" :aria-valuemax="steps.length" aria-valuemin="1"><i v-for="(item,index) in steps" :key="item" :class="{ passed:index <= steps.indexOf(step) }"/><small>{{ steps.indexOf(step)+1 }} / {{ steps.length }}</small></div><button v-if="steps.indexOf(step)>0" :disabled="busy || !initialized" @click="back"><ArrowLeft :size="16"/>{{ t(key('back')) }}</button><button :disabled="busy || !initialized || !!storagePreview" class="primary" @click="next">{{ t(key(step === 'modules' ? 'finish' : 'next')) }}<ArrowRight :size="16"/></button></footer>
        </template>
      </section>
    </div>
  </Teleport>
</template>
<style scoped>
.onboarding-mask{position:fixed;inset:0;z-index:1600;background:color-mix(in srgb,var(--copper-bg) 72%,transparent);backdrop-filter:blur(38px) saturate(.65);-webkit-backdrop-filter:blur(38px) saturate(.65);color:var(--copper-text);overflow:auto}.onboarding-panel{min-height:100%;box-sizing:border-box;display:flex;flex-direction:column;align-items:center;padding:clamp(24px,5vh,64px) clamp(20px,6vw,100px) 28px;outline:none;gap:28px}.launcher-preview{display:flex;flex-shrink:0;width:min(800px,88vw);height:clamp(200px,34vh,360px);overflow:hidden;border-radius:var(--copper-radius-card,16px);background:var(--copper-bg);box-shadow:var(--copper-shadow-card,0 18px 64px #0003);pointer-events:none}.launcher-preview aside{width:66px;background:var(--copper-surface);display:flex;flex-direction:column;align-items:center;gap:26px;padding:18px 0;color:var(--copper-text-secondary)}.launcher-preview aside svg{width:19px}.preview-workspace{flex:1;min-width:0}.preview-toolbar{display:flex;align-items:center;gap:8px;padding:15px 20px;font-size:12px;color:var(--copper-text-secondary)}.preview-toolbar span{flex:1}.preview-toolbar i{width:7px;height:7px;border-radius:50%;background:var(--copper-hover)}.preview-body{padding:12px 26px}.preview-heading{display:flex;align-items:center;gap:12px;font-size:22px}.preview-grid{display:grid;grid-template-columns:1.1fr 1fr;gap:20px;margin-top:20px}.preview-feature,.preview-list{border-radius:var(--copper-radius-card,16px);background:var(--copper-surface);padding:22px;box-shadow:var(--copper-shadow-card)}.preview-feature{display:flex;gap:18px;align-items:center;color:var(--copper-accent)}.preview-feature span{color:var(--copper-text);font-size:14px}.preview-list>div{display:flex;gap:12px;align-items:center;margin-bottom:12px}.preview-list span{width:28px;height:28px;border-radius:8px;background:var(--copper-hover)}.preview-list i{height:8px;width:70%;border-radius:8px;background:var(--copper-hover)}.preview-list button{width:100%;padding:8px}.copy{width:min(800px,100%);text-align:center;flex:1}.copy>small{color:var(--copper-accent);font-weight:600}h1{font-size:clamp(26px,3vw,38px);margin:12px 0 14px;letter-spacing:-.025em}h2{font-size:20px}p{line-height:1.8;color:var(--copper-text-secondary)}.description{max-width:660px;margin:0 auto 20px;font-size:16px}.details{max-height:28vh;overflow:auto;text-align:start;padding:4px 8px}.controls{display:flex;gap:12px;justify-content:center;flex-wrap:wrap}button{display:inline-flex;align-items:center;justify-content:center;gap:8px;padding:12px 18px;border:0;border-radius:var(--copper-radius-control,16px);background:var(--copper-surface);color:inherit;font:inherit;cursor:pointer;box-shadow:var(--copper-shadow-card)}button.primary,button[aria-pressed=true],.preview-list button{background:var(--copper-accent);color:var(--copper-accent-foreground)}button:disabled{opacity:.45;cursor:not-allowed}button:focus-visible,input:focus-visible{outline:2px solid var(--copper-accent);outline-offset:3px}footer{display:flex;width:min(1000px,100%);align-items:center;gap:12px;flex-shrink:0;flex-wrap:wrap;margin-top:auto;padding-top:8px}.progress{flex:1;display:flex;align-items:center;justify-content:center;gap:7px;min-width:120px}.progress i{width:7px;height:7px;background:var(--copper-hover);border-radius:50%}.progress i.passed{background:var(--copper-accent)}.progress small{margin-inline-start:8px;color:var(--copper-text-secondary)}.module-row{display:flex;align-items:center;gap:12px;padding:14px;border-radius:var(--copper-radius-card,16px);background:var(--copper-surface);margin-bottom:10px;overflow-wrap:anywhere}.module-row span{flex:1;display:grid;gap:5px;min-width:0}small{color:var(--copper-text-secondary)}dl{font-size:13px;overflow-wrap:anywhere}dt{font-weight:600}dd{margin:4px 0 12px}.brief{display:flex;text-align:start;gap:24px;margin:26px 0;padding:20px;background:var(--copper-surface);border-radius:var(--copper-radius-card,16px)}.brief img{object-fit:contain}.error{padding:12px;border-radius:var(--copper-radius-card,16px);background:var(--copper-surface)}@media(max-width:600px){.onboarding-panel{padding:24px 18px;gap:22px}.launcher-preview{width:100%;height:210px}.launcher-preview aside{width:44px;gap:20px}.preview-body{padding:8px 14px}.preview-heading{font-size:17px}.preview-feature{padding:16px}.preview-feature svg{width:28px}.preview-grid{grid-template-columns:1fr;margin-top:16px}.preview-list{display:none}.preview-feature span{font-size:13px}.details{max-height:32vh}footer{gap:8px}footer button{padding:10px 12px;font-size:12px}.progress{order:-1;flex-basis:100%;margin-bottom:8px}.module-row{flex-wrap:wrap}.brief{flex-direction:column}}@media(prefers-reduced-motion:no-preference){.launcher-preview{animation:arrive 240ms ease-out}@keyframes arrive{from{opacity:0;transform:translateY(12px)}to{opacity:1;transform:none}}}
</style>
