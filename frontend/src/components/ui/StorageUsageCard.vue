<script setup lang="ts">
import { computed } from "vue";
import { RotateCw } from "@lucide/vue";
import { useI18n, useLocaleRef } from "../../i18n";
import { formatBytes } from "../../api/download";
import { useStorageScan } from "../../composables/useStorageScan";
import CoButton from "./CoButton.vue";

// 稳定配色：按分类 id 取色，这样分类顺序变化或某类暂时无值时色彩不跳。
const COLORS: Record<string, string> = {
  content: "var(--copper-accent)",
  downloads: "var(--copper-success)",
  loaders: "var(--copper-warning)",
  index: "color-mix(in srgb, var(--copper-accent) 52%, var(--copper-success))",
  logs: "color-mix(in srgb, var(--copper-accent) 52%, var(--copper-warning))",
  game: "color-mix(in srgb, var(--copper-text-secondary) 82%, var(--copper-accent))",
  other: "color-mix(in srgb, var(--copper-text-secondary) 46%, var(--copper-surface-3))",
};

const scan = useStorageScan();
const { t } = useI18n();
const locale = useLocaleRef();

const loading = computed(() => scan.snapshot.value === null);
const totalBytes = computed(() => scan.totalBytes.value);
const files = computed(() => scan.files.value);
const showSpinner = computed(() => scan.scanning.value || (!scan.ready.value && !scan.failed.value));

const segments = computed(() => {
  const sum = totalBytes.value ?? 0;
  let offset = 0;
  return scan.categories.value.map((item) => {
    const bytes = item.bytes;
    const fraction = sum > 0 && bytes !== null ? Math.max(0, bytes) / sum : 0;
    const segment = {
      id: item.id,
      bytes,
      state: item.state,
      color: COLORS[item.id] ?? "var(--copper-text-secondary)",
      offset,
      fraction,
      percent: sum > 0 && bytes !== null ? Math.round(fraction * 100) : null,
      subtitle: "",
    };
    offset += fraction;
    return segment;
  });
});

const ringStyle = computed<Record<string, string> | undefined>(() => {
  if (!scan.ready.value) {
    return showSpinner.value ? undefined : { background: "var(--copper-surface-3)" };
  }
  const pieces = segments.value
    .filter((item) => (item.bytes ?? 0) > 0)
    .map((item) => {
      const start = item.offset * 100;
      const end = (item.offset + item.fraction) * 100;
      return item.color + " " + start.toFixed(2) + "% " + end.toFixed(2) + "%";
    });
  if (!pieces.length) return { background: "var(--copper-surface-3)" };
  return { background: "conic-gradient(" + pieces.join(", ") + ")" };
});

const timeLabel = computed(() => {
  const started = scan.finishedAt.value;
  if (!started) return "";
  const at = new Date(started);
  const sameDay = at.toDateString() === new Date().toDateString();
  const format = new Intl.DateTimeFormat(
    locale.value,
    sameDay
      ? { hour: "2-digit", minute: "2-digit" }
      : { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" },
  );
  return format.format(at);
});

const statusTone = computed(() => {
  if (scan.failed.value) return "failed";
  if (scan.scanning.value) return "running";
  if (scan.ready.value) return "ready";
  return "idle";
});

const statusText = computed(() => {
  if (scan.failed.value) return t("settings.storage.usage.scanFailed");
  if (scan.scanning.value) return t("settings.storage.usage.scanning");
  if (scan.ready.value) return t("settings.storage.usage.scannedAt", { time: timeLabel.value });
  return t("settings.storage.usage.scanIdle");
});

const activeCategoryLabel = computed(() => {
  const progress = scan.progress.value;
  if (!progress) return "";
  return (
    t("settings.storage.usage.category." + progress.category) +
    " · " +
    t("settings.storage.usage.scanFiles", { count: progress.files })
  );
});

const statusDetail = computed(() =>
  scan.scanning.value && activeCategoryLabel.value ? activeCategoryLabel.value : "",
);
</script>

<template>
  <section class="usage-card" :aria-busy="loading || scan.scanning.value">
    <header class="usage-card__head">
      <div class="usage-card__heading">
        <h2>{{ t("settings.storage.usage.title") }}</h2>
        <p class="usage-card__status" role="status" aria-live="polite">
          <span class="usage-card__state" :class="'is-' + statusTone" aria-hidden="true" />
          <span>{{ statusText }}</span>
          <small v-if="statusDetail">{{ statusDetail }}</small>
        </p>
      </div>
      <CoButton
        variant="ghost"
        size="sm"
        class="usage-card__refresh"
        :disabled="scan.scanning.value"
        :aria-label="t('settings.storage.usage.retry')"
        @click="scan.rescan(true)"
      >
        <RotateCw :size="15" :class="{ 'usage-card__refresh-icon': true, 'is-spinning': scan.scanning.value }" />
      </CoButton>
    </header>

    <div v-if="loading" class="usage-card__loading" aria-hidden="true">
      <span class="usage-card__loading-ring" />
      <div class="usage-card__loading-lines"><span /><span /><span /><span /></div>
    </div>

    <div v-else class="usage-card__body">
      <div class="usage-card__chart">
        <div class="usage-card__ring" :class="{ 'is-scanning': showSpinner, 'is-ready': scan.ready.value }" :style="ringStyle" />
        <div class="usage-card__center">
          <template v-if="scan.ready.value">
            <strong class="usage-card__total">{{ formatBytes(totalBytes ?? 0) }}</strong>
            <span class="usage-card__caption">{{ t("settings.storage.usage.total") }}</span>
            <small class="usage-card__caption usage-card__files">{{ t("settings.storage.usage.files", { count: files }) }}</small>
          </template>
          <template v-else-if="scan.failed.value">
            <span class="usage-card__failed">{{ t("settings.storage.usage.scanFailed") }}</span>
          </template>
          <template v-else>
            <span class="usage-card__scanning">{{ t("settings.storage.usage.scanning") }}</span>
            <span class="usage-card__dots" aria-hidden="true"><i /><i /><i /></span>
          </template>
        </div>
      </div>

      <ul class="usage-card__legend">
        <li v-for="item in segments" :key="item.id" :class="{ 'is-pending': item.bytes === null }">
          <i class="usage-card__swatch" :style="{ background: item.color }" />
          <span class="usage-card__name">{{ t("settings.storage.usage.category." + item.id) }}</span>
          <Transition name="usage-value" mode="out-in">
            <span v-if="item.bytes !== null" :key="'value-' + item.bytes" class="usage-card__value">{{ formatBytes(item.bytes) }}</span>
            <span v-else :key="'pending'" class="usage-card__placeholder" :class="{ 'is-running': item.state === 'running' }" />
          </Transition>
          <small class="usage-card__percent">{{ item.percent === null ? "" : t("settings.storage.usage.percent", { percent: item.percent }) }}</small>
        </li>
        <li v-if="!segments.length" class="usage-card__empty">{{ t("settings.storage.usage.empty") }}</li>
      </ul>
    </div>
  </section>
</template>

<style scoped>
.usage-card{
  display:grid;
  gap:var(--copper-space-4);
  padding:var(--copper-space-5);
  background:var(--copper-surface);
  border-radius:var(--copper-radius-card);
  box-shadow:var(--copper-shadow-card);
}
.usage-card__head{display:flex;align-items:flex-start;justify-content:space-between;gap:var(--copper-space-3)}
.usage-card__heading{display:grid;gap:4px;min-width:0}
h2{margin:0;font-size:var(--copper-font-size-md);font-weight:600}
.usage-card__status{margin:0;display:flex;align-items:center;flex-wrap:wrap;gap:6px;min-height:16px;font-size:var(--copper-font-size-xs);color:var(--copper-text-secondary)}
.usage-card__status small{color:var(--copper-text-secondary);opacity:.82;font-variant-numeric:tabular-nums}
.usage-card__state{width:6px;height:6px;border-radius:50%;background:var(--copper-text-secondary);flex:0 0 auto}
.usage-card__state.is-running{background:var(--copper-accent);animation:state-breathe 1.4s ease-in-out infinite}
.usage-card__state.is-ready{background:var(--copper-success)}
.usage-card__state.is-failed{background:var(--copper-danger)}
.usage-card__refresh{flex:0 0 auto}
.usage-card__refresh-icon{transition:transform var(--copper-motion-component-enter) var(--copper-easing-jelly)}
.usage-card__refresh-icon.is-spinning{animation:icon-spin 1.1s linear infinite}
.usage-card__body{display:grid;grid-template-columns:minmax(160px,220px) minmax(0,1fr);align-items:center;gap:var(--copper-space-5)}
.usage-card__chart{position:relative;width:min(100%,200px);aspect-ratio:1;display:grid;place-items:center;margin:auto}
.usage-card__ring{position:absolute;inset:0;border-radius:50%;background:var(--copper-surface-3)}
.usage-card__ring.is-ready{animation:ring-in 420ms var(--copper-easing-jelly)}
.usage-card__ring.is-scanning{
  background:conic-gradient(from 0deg,transparent 0deg,color-mix(in srgb,var(--copper-accent) 88%,transparent) 74deg,transparent 142deg);
  animation:ring-spin 1.15s linear infinite;
}
.usage-card__center{position:relative;z-index:1;width:70%;aspect-ratio:1;border-radius:50%;background:var(--copper-surface);display:flex;flex-direction:column;align-items:center;justify-content:center;gap:2px;padding:var(--copper-space-3);text-align:center}
.usage-card__total{font-size:var(--copper-font-size-lg);font-weight:650;font-variant-numeric:tabular-nums}
.usage-card__caption{font-size:var(--copper-font-size-xs);color:var(--copper-text-secondary)}
.usage-card__files{opacity:.72}
.usage-card__scanning{font-size:var(--copper-font-size-sm);color:var(--copper-text-secondary)}
.usage-card__failed{font-size:var(--copper-font-size-sm);color:var(--copper-danger)}
.usage-card__dots{display:flex;gap:4px;margin-top:2px}
.usage-card__dots i{width:4px;height:4px;border-radius:50%;background:var(--copper-accent);opacity:.4;animation:dot-bounce 1.2s ease-in-out infinite}
.usage-card__dots i:nth-child(2){animation-delay:.16s}
.usage-card__dots i:nth-child(3){animation-delay:.32s}
.usage-card__legend{list-style:none;margin:0;padding:0;display:grid;gap:10px;min-width:0}
.usage-card__legend li{display:grid;grid-template-columns:9px minmax(0,1fr) auto minmax(38px,auto);align-items:center;gap:10px;min-width:0;font-size:var(--copper-font-size-sm)}
.usage-card__swatch{width:9px;height:9px;border-radius:50%;flex:0 0 auto}
.usage-card__name{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--copper-text)}
.usage-card__legend li.is-pending .usage-card__name{color:var(--copper-text-secondary)}
.usage-card__value{font-variant-numeric:tabular-nums}
.usage-card__percent{font-size:var(--copper-font-size-xs);color:var(--copper-text-secondary);font-variant-numeric:tabular-nums;text-align:right;min-width:38px}
.usage-card__placeholder{position:relative;display:block;width:56px;height:8px;border-radius:var(--copper-radius-full);background:var(--copper-surface-3);overflow:hidden;justify-self:end}
.usage-card__placeholder::after{content:"";position:absolute;inset:0;transform:translateX(-100%);background:linear-gradient(90deg,transparent,color-mix(in srgb,var(--copper-text-secondary) 22%,transparent),transparent);animation:placeholder-sweep 1.5s ease-in-out infinite}
.usage-card__placeholder.is-running{background:color-mix(in srgb,var(--copper-accent) 22%,var(--copper-surface-3))}
.usage-card__empty{grid-column:1/-1;color:var(--copper-text-secondary);font-size:var(--copper-font-size-sm)}
.usage-card__loading{display:grid;grid-template-columns:minmax(160px,220px) minmax(0,1fr);align-items:center;gap:var(--copper-space-5)}
.usage-card__loading-ring{width:min(100%,200px);aspect-ratio:1;margin:auto;border-radius:50%;background:var(--copper-surface-2);animation:loading-pulse 1.2s ease-in-out infinite alternate}
.usage-card__loading-lines{display:grid;gap:12px}
.usage-card__loading-lines span{height:10px;border-radius:var(--copper-radius-full);background:var(--copper-surface-2);animation:loading-pulse 1.2s ease-in-out infinite alternate}
.usage-card__loading-lines span:first-child{width:52%}
.usage-card__loading-lines span:nth-child(2){width:84%}
.usage-card__loading-lines span:nth-child(3){width:70%}
.usage-card__loading-lines span:last-child{width:62%}
.usage-value-enter-active,.usage-value-leave-active{transition:opacity 160ms ease,transform 160ms ease}
.usage-value-enter-from{opacity:0;transform:translateY(3px)}
.usage-value-leave-to{opacity:0;transform:translateY(-3px)}
@keyframes ring-spin{to{transform:rotate(360deg)}}
@keyframes ring-in{from{transform:scale(.96);opacity:.4}}
@keyframes icon-spin{to{transform:rotate(360deg)}}
@keyframes state-breathe{0%,100%{opacity:.45}50%{opacity:1}}
@keyframes dot-bounce{0%,80%,100%{opacity:.28;transform:translateY(0)}40%{opacity:1;transform:translateY(-3px)}}
@keyframes placeholder-sweep{to{transform:translateX(100%)}}
@keyframes loading-pulse{to{opacity:.5}}
@media(max-width:560px){
  .usage-card{padding:var(--copper-space-4)}
  .usage-card__body,.usage-card__loading{grid-template-columns:1fr;gap:var(--copper-space-4)}
  .usage-card__chart,.usage-card__loading-ring{width:150px}
  .usage-card__legend li{grid-template-columns:9px minmax(0,1fr) auto}
  .usage-card__percent{display:none}
}
@media(prefers-reduced-motion:reduce){
  .usage-card__ring.is-scanning,.usage-card__refresh-icon.is-spinning,.usage-card__state.is-running,.usage-card__dots i,.usage-card__placeholder::after{animation:none}
  .usage-card__ring.is-scanning{background:conic-gradient(from 0deg,transparent 0deg,color-mix(in srgb,var(--copper-accent) 55%,transparent) 120deg,transparent 220deg)}
  .usage-card__dots i{opacity:.75;transform:none}
}
</style>
