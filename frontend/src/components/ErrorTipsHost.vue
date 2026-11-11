<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from 'vue';
import { AlertTriangle, Copy, Settings, ListChecks, X } from '@lucide/vue';
import { useRouter } from 'vue-router';
import { activeErrorReport, contributedText, diagnosticText } from '../services/errtips';
import { errtipText } from '../services/errtips-messages';
import { t, useLocaleRef } from '../i18n';
import { acquireModal, activeModalId, isModalActive } from '../services/modalCoordinator';
const locale = useLocaleRef();
let releaseModal: (() => void) | undefined;
const router = useRouter();
function navigate(target: 'settings' | 'downloads') {
  void router.push(target === 'settings' ? '/settings' : '/downloads');
  close();
}
const text = (key: string) => errtipText(locale.value, key);
const panel = ref<HTMLElement | null>(null);
const feedback = ref('');
let previousFocus: HTMLElement | null = null;
const report = activeErrorReport;
const diagnostic = computed(() => report.value ? diagnosticText(report.value) : '');
const contribution = computed(() => report.value ? contributedText(report.value) : undefined);
watch(report, async value => {
  feedback.value = '';
  if (value) { if (!releaseModal) releaseModal = acquireModal('error-tips', 10); previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null; await nextTick(); if (isModalActive('error-tips')) panel.value?.focus(); }
  else { releaseModal?.(); releaseModal = undefined; if (!activeModalId.value) previousFocus?.focus(); }
});
onUnmounted(() => releaseModal?.());
function close() { report.value = null; }
watch(activeModalId, async id => { if (id === 'error-tips') { await nextTick(); panel.value?.focus(); } });
function keydown(event: KeyboardEvent) {
  if (!isModalActive('error-tips')) return;
  if (event.key === 'Escape') { event.stopPropagation(); close(); }
  if (event.key !== 'Tab') return;
  const nodes = panel.value?.querySelectorAll<HTMLElement>('button, textarea, [tabindex="0"]');
  if (!nodes?.length) return;
  const first = nodes[0]!; const last = nodes[nodes.length - 1]!;
  if (event.shiftKey && (document.activeElement === first || document.activeElement === panel.value)) { event.preventDefault(); last.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
}
async function copy() {
  try { await navigator.clipboard.writeText(diagnostic.value); feedback.value = text('copied'); }
  catch { feedback.value = text('copy_failed'); }
}
</script>
<template>
  <Teleport to="body">
    <div v-if="report && isModalActive('error-tips')" class="errtips-mask" @keydown="keydown">
      <section ref="panel" class="errtips-panel" role="dialog" aria-modal="true" aria-labelledby="errtips-title" aria-describedby="errtips-body" tabindex="-1">
        <header><AlertTriangle :size="22" /><h2 id="errtips-title">{{ contribution ? t(contribution.titleKey) : text('title') }}</h2><button :aria-label="text('close')" @click="close"><X :size="18" /></button></header>
        <p id="errtips-body">{{ contribution ? t(contribution.bodyKey) : text(report.rule) }}</p>
        <p v-if="report.context.host?.endsWith('github.com') || report.context.host?.endsWith('githubusercontent.com')">{{ text('github') }}</p>
        <p class="errtips-muted">{{ text('privacy') }}</p>
        <label for="errtips-diagnostic">{{ text('evidence') }}</label>
        <textarea id="errtips-diagnostic" :value="diagnostic" readonly spellcheck="false" />
        <p v-if="feedback" role="status">{{ feedback }}</p>
        <footer><button @click="navigate('downloads')"><ListChecks :size="15" />{{ text('tasks') }}</button><button @click="navigate('settings')"><Settings :size="15" />{{ text('settings') }}</button><button @click="copy"><Copy :size="15" />{{ text('copy') }}</button><button @click="close">{{ text('close') }}</button></footer>
      </section>
    </div>
  </Teleport>
</template>
<style scoped>
.errtips-mask { position:fixed; inset:0; z-index:1200; display:grid; place-items:center; padding:20px; background:rgb(0 0 0 / .34); backdrop-filter:blur(6px); }
.errtips-panel { width:min(620px,100%); max-height:90dvh; overflow:auto; box-sizing:border-box; padding:24px; border-radius:var(--copper-radius-lg,16px); background:var(--copper-surface); color:var(--copper-text); box-shadow:var(--copper-shadow); outline:none; }
header,footer { display:flex; align-items:center; gap:12px; } header h2 { flex:1; font-size:18px; margin:0; } header>svg { color:var(--copper-danger); }
p { line-height:1.6; overflow-wrap:anywhere; } .errtips-muted { color:var(--copper-text-secondary); font-size:12px; }
label { display:block; margin-bottom:8px; } textarea { box-sizing:border-box; width:100%; min-height:180px; resize:vertical; border:0; border-radius:8px; padding:12px; background:var(--copper-bg); color:inherit; font-size:12px; }
footer { justify-content:flex-end; margin-top:18px; flex-wrap:wrap; } button { display:inline-flex; align-items:center; justify-content:center; gap:7px; border:0; border-radius:8px; padding:9px 12px; color:inherit; background:color-mix(in srgb,var(--copper-text) 7%,var(--copper-surface)); cursor:pointer; } button:hover { background:color-mix(in srgb,var(--copper-accent) 18%,var(--copper-surface)); } button:focus-visible,textarea:focus-visible { outline:2px solid var(--copper-accent); outline-offset:2px; }
@media(prefers-reduced-motion:reduce) { .errtips-mask { backdrop-filter:none; } }
@media(max-width:600px) { .errtips-panel { padding:18px; } .errtips-mask { padding:10px; } }
</style>
