<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "../../i18n";
import { formatBytes } from "../../api/download";
import type { StorageUsage } from "../../api/storage";

const props = defineProps<{ usage: StorageUsage | null; loading: boolean }>();
const { t } = useI18n();
const colors = ["var(--copper-accent)", "var(--copper-success)", "var(--copper-warning)", "#4d9de0", "#a66dd4", "#7f8998", "#b2b8c2"];
const total = computed(() => props.usage?.totalBytes ?? 0);
const segments = computed(() => {
  const categories = props.usage?.categories ?? [];
  let offset = 0;
  return categories.map((item, index) => {
    const fraction = total.value > 0 ? Math.max(0, item.bytes) / total.value : 0;
    const segment = { ...item, offset, fraction, color: colors[index % colors.length] };
    offset += fraction;
    return segment;
  });
});
const ring = computed(() => {
  if (total.value === 0) return "var(--copper-surface-3)";
  let stop = 0;
  const pieces = segments.value.map((item) => {
    const start = stop;
    stop += item.fraction * 100;
    return `${item.color} ${start}% ${stop}%`;
  });
  return `conic-gradient(${pieces.join(", ")})`;
});
</script>

<template>
  <section class="usage-card" :aria-busy="loading">
    <h2>{{ t("settings.storage.usage.title") }}</h2>
    <div v-if="loading" class="usage-card__loading" aria-hidden="true"><span/><span/><span/></div>
    <template v-else-if="usage">
      <div class="usage-card__body">
        <div class="usage-card__chart" :style="{ background: ring }" role="img" :aria-label="`${t('settings.storage.usage.total')}: ${formatBytes(total)}`">
          <div class="usage-card__center"><strong>{{ formatBytes(total) }}</strong><span>{{ t("settings.storage.usage.total") }}</span></div>
        </div>
        <ul class="usage-card__legend">
          <li v-for="item in segments" :key="item.id">
            <i :style="{ background: item.color }"/><span class="usage-card__name">{{ t(`settings.storage.usage.category.${item.id}`) }}</span>
            <span class="usage-card__value">{{ formatBytes(item.bytes) }}</span>
            <small>{{ t("settings.storage.usage.percent", { percent: total ? Math.round(item.bytes / total * 100) : 0 }) }}</small>
          </li>
          <li v-if="!segments.length" class="usage-card__empty">{{ t("settings.storage.usage.empty") }}</li>
        </ul>
      </div>
    </template>
    <div v-else class="usage-card__error" role="status">{{ t("settings.storage.usage.error") }}</div>
  </section>
</template>

<style scoped>
.usage-card{padding:var(--copper-space-4);background:var(--copper-surface);border-radius:var(--copper-radius-card);box-shadow:var(--copper-shadow-card)}
h2{margin:0 0 var(--copper-space-4);font-size:var(--copper-font-size-md);font-weight:600}
.usage-card__body{display:grid;grid-template-columns:minmax(160px,220px) 1fr;align-items:center;gap:var(--copper-space-5)}
.usage-card__chart{width:min(100%,200px);aspect-ratio:1;border-radius:50%;display:grid;place-items:center;margin:auto}
.usage-card__center{width:70%;aspect-ratio:1;border-radius:50%;background:var(--copper-surface);display:flex;flex-direction:column;align-items:center;justify-content:center;gap:4px}
.usage-card__center strong{font-size:var(--copper-font-size-lg);font-weight:650}.usage-card__center span{font-size:var(--copper-font-size-xs);color:var(--copper-text-secondary)}
.usage-card__legend{list-style:none;margin:0;padding:0;display:grid;gap:10px}.usage-card__legend li{display:grid;grid-template-columns:10px minmax(0,1fr) auto auto;align-items:center;gap:10px;min-width:0;font-size:var(--copper-font-size-sm)}.usage-card__legend i{width:9px;height:9px;border-radius:50%}.usage-card__name{overflow-wrap:anywhere}.usage-card__value{font-variant-numeric:tabular-nums}.usage-card__legend small{color:var(--copper-text-secondary);font-size:var(--copper-font-size-xs)}.usage-card__empty,.usage-card__error{grid-column:1/-1;color:var(--copper-text-secondary);padding:var(--copper-space-3)}
.usage-card__loading{height:176px;display:grid;gap:10px;align-content:center}.usage-card__loading span{height:12px;border-radius:var(--copper-radius-full);background:var(--copper-surface-2);animation:pulse 1.2s ease-in-out infinite alternate}.usage-card__loading span:first-child{width:40%}.usage-card__loading span:nth-child(2){width:75%}.usage-card__loading span:last-child{width:58%}@keyframes pulse{to{opacity:.45}}
@media(max-width:560px){.usage-card__body{grid-template-columns:1fr;gap:var(--copper-space-4)}.usage-card__chart{width:150px}.usage-card__legend li{grid-template-columns:10px minmax(0,1fr) auto}.usage-card__legend small{grid-column:2/-1}}
</style>
