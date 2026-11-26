<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { RotateCw } from "@lucide/vue";
import { useI18n, useLocaleRef } from "../../i18n";
import { formatBytes } from "../../api/download";
import { useStorageScan } from "../../composables/useStorageScan";
import { STORAGE_CATEGORIES as CATEGORIES, STORAGE_GROUPS as GROUPS, storageCategoryColor } from "../../services/storage-categories";
import CoButton from "./CoButton.vue";

// 存储用量只统计可清理的七类数据。分类声明在 services/storage-categories.ts，
// 这里只负责把后端结果合并进图表；配色用 --copper-chart-N 而不是强调色系 ——
// 强调色只有一个，七分类都从它派生会糊成一片灰。

/** 环的几何：240 视框里居中一条 r=88、线宽 34 的圆环。 */
const RADIUS = 88;
const STROKE = 34;

const scan = useStorageScan();
const { t } = useI18n();
const locale = useLocaleRef();

const hovered = ref<string | null>(null);
const grown = ref(false);
onMounted(() => requestAnimationFrame(() => { grown.value = true; }));

const loading = computed(() => scan.snapshot.value === null);
const scanning = computed(() => scan.scanning.value);
const ready = computed(() => scan.ready.value);
const failed = computed(() => scan.failed.value);
const total = computed(() => scan.totalBytes.value);
const files = computed(() => scan.files.value);

/** 先按真实占比分角度，再给非零小扇形托底，避免 0.3% 的分类在环上彻底消失。 */
function spreadDegrees(fractions: number[]): number[] {
  const degrees = fractions.map((value) => Math.max(0, value) * 360);
  const MIN = 9;
  for (let pass = 0; pass < 2; pass += 1) {
    let deficit = 0;
    let excess = 0;
    for (const degree of degrees) {
      if (degree > 0 && degree < MIN) deficit += MIN - degree;
      else if (degree > MIN) excess += degree - MIN;
    }
    if (deficit <= 0 || excess <= 0) break;
    const scale = Math.min(1, deficit / excess);
    for (let index = 0; index < degrees.length; index += 1) {
      const degree = degrees[index]!;
      if (degree > 0 && degree < MIN) degrees[index] = Math.min(MIN, degree + (MIN - degree) * scale);
      else if (degree > MIN) degrees[index] = degree - (degree - MIN) * scale;
    }
  }
  return degrees;
}

function percentLabel(fraction: number): string {
  const percent = fraction * 100;
  const text = percent >= 10 ? String(Math.round(percent)) : percent.toFixed(1);
  return t("settings.storage.usage.percent", { percent: text });
}

/**
 * 环上每一段的长度与起点。
 *
 * 分母优先用后端快照的总量：开始新扫描时后端会保留上一轮总量，所以扫盘过程中
 * 弧线只会在小范围内变化，不会因为「先扫完的小分类」先占满整圈而跳一下。
 */
const segments = computed(() => {
  const known = new Map(scan.categories.value.map((item) => [item.id, item]));
  const list = CATEGORIES.map((def) => {
    const item = known.get(def.id);
    return {
      id: def.id as string,
      tone: def.tone,
      color: storageCategoryColor(def.tone),
      bytes: item?.bytes ?? null,
      state: item?.state ?? "pending",
    };
  });
  const sumKnown = list.reduce((sum, item) => sum + (item.bytes ?? 0), 0);
  const denominator = ready.value ? (total.value ?? sumKnown) : (scan.snapshotTotal.value ?? sumKnown);
  const fractions = list.map((item) =>
    denominator > 0 && item.bytes !== null ? Math.max(0, item.bytes) / denominator : 0,
  );
  const degrees = spreadDegrees(fractions);
  let cursor = 0;
  return list.map((item, index) => {
    const degree = degrees[index] ?? 0;
    const fraction = fractions[index] ?? 0;
    const start = cursor;
    cursor += degree;
    // pathLength=100 把周长归一成百分比单位：1 度 = 1/3.6 单位。
    const span = degree / 3.6;
    const gap = degree > 0 ? Math.min(0.7, span * 0.18) : 0;
    return {
      ...item,
      fraction,
      percent: fraction > 0 ? percentLabel(fraction) : null,
      dash: grown.value ? Math.max(0, span - gap) : 0,
      offset: grown.value ? -(start / 3.6 + gap / 2) : 0,
    };
  });
});

const groups = computed(() =>
  GROUPS.map((group) => ({
    id: group.id,
    rows: group.ids
      .map((id) => segments.value.find((item) => item.id === id))
      .filter((item): item is (typeof segments.value)[number] => Boolean(item)),
  })),
);

const timeLabel = computed(() => {
  const finished = scan.finishedAt.value;
  if (!finished) return "";
  const at = new Date(finished);
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
  if (failed.value) return "failed";
  if (scanning.value) return "running";
  if (ready.value) return "ready";
  return "idle";
});

const statusDetail = computed(() => {
  const progress = scan.progress.value;
  if (!scanning.value || !progress) return "";
  return t("settings.storage.usage.scanningCategory", {
    category: t("settings.storage.usage.category." + progress.category),
  });
});

const statusText = computed(() => {
  if (failed.value) return t("settings.storage.usage.scanFailed");
  if (scanning.value) return t("settings.storage.usage.scanning");
  if (ready.value) return t("settings.storage.usage.scannedAt", { time: timeLabel.value });
  return t("settings.storage.usage.scanIdle");
});

function highlight(id: string | null) {
  hovered.value = id;
}
</script>

<template>
  <section class="usage-card" :aria-busy="loading || scanning">
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
        :disabled="scanning"
        :aria-label="t('settings.storage.usage.retry')"
        @click="scan.rescan(true)"
      >
        <RotateCw :size="15" />
      </CoButton>
    </header>

    <div class="usage-card__body">
      <div class="usage-card__chart">
        <svg class="usage-card__ring" viewBox="0 0 240 240" role="img" :aria-label="statusText">
          <circle
            class="usage-card__track"
            :cx="120"
            :cy="120"
            :r="RADIUS"
            fill="none"
            :stroke-width="STROKE"
          />
          <circle
            v-for="segment in segments"
            :key="segment.id"
            class="usage-card__arc"
            :class="{ 'is-hovered': hovered === segment.id }"
            :cx="120"
            :cy="120"
            :r="RADIUS"
            pathLength="100"
            fill="none"
            :stroke="segment.color"
            :stroke-width="hovered === segment.id ? STROKE + 4 : STROKE"
            :stroke-dasharray="segment.dash + ' ' + (100 - segment.dash)"
            :stroke-dashoffset="segment.offset"
            transform="rotate(-90 120 120)"
            @pointerenter="highlight(segment.id)"
            @pointerleave="highlight(null)"
          />
        </svg>
        <div class="usage-card__center">
          <template v-if="ready">
            <strong class="usage-card__total">{{ formatBytes(total ?? 0) }}</strong>
            <span class="usage-card__caption">{{ t("settings.storage.usage.total") }}</span>
            <small class="usage-card__caption usage-card__files">{{ t("settings.storage.usage.files", { count: files }) }}</small>
          </template>
          <template v-else-if="failed">
            <span class="usage-card__failed">{{ t("settings.storage.usage.scanFailed") }}</span>
          </template>
          <template v-else>
            <span class="usage-card__scanning">{{ t("settings.storage.usage.scanning") }}</span>
          </template>
        </div>
      </div>

      <div class="usage-card__legend">
        <section v-for="group in groups" :key="group.id" class="usage-card__group">
          <h3>{{ t("settings.storage.usage.group." + group.id) }}</h3>
          <ul>
            <li
              v-for="row in group.rows"
              :key="row.id"
              :class="{ 'is-pending': row.bytes === null, 'is-hovered': hovered === row.id }"
              @pointerenter="highlight(row.id)"
              @pointerleave="highlight(null)"
            >
              <i class="usage-card__swatch" :style="{ background: row.color }" aria-hidden="true" />
              <span class="usage-card__name">
                <span>{{ t("settings.storage.usage.category." + row.id) }}</span>
                <small>{{ t("settings.storage.usage.hint." + row.id) }}</small>
              </span>
              <Transition name="usage-value" mode="out-in">
                <strong v-if="row.bytes !== null" :key="'value-' + row.bytes" class="usage-card__value">{{ formatBytes(row.bytes) }}</strong>
                <span v-else :key="'pending'" class="usage-card__placeholder" />
              </Transition>
              <small class="usage-card__percent">{{ row.percent ?? "" }}</small>
            </li>
          </ul>
        </section>
        <p v-if="!segments.length" class="usage-card__empty">{{ t("settings.storage.usage.empty") }}</p>
      </div>
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
.usage-card__status small{color:var(--copper-text-secondary);opacity:.82}
.usage-card__state{width:6px;height:6px;border-radius:50%;background:var(--copper-text-secondary);flex:0 0 auto}
.usage-card__state.is-running{background:var(--copper-accent)}
.usage-card__state.is-ready{background:var(--copper-success)}
.usage-card__state.is-failed{background:var(--copper-danger)}
.usage-card__body{display:grid;grid-template-columns:minmax(180px,236px) minmax(0,1fr);align-items:center;gap:var(--copper-space-5)}
.usage-card__chart{position:relative;width:min(100%,236px);aspect-ratio:1;display:grid;place-items:center;margin:auto}
.usage-card__ring{width:100%;height:100%;overflow:visible}
.usage-card__track{stroke:var(--copper-chart-track)}
.usage-card__arc{
  cursor:default;
  transition:
    stroke-dasharray 480ms cubic-bezier(0.22,1,0.36,1),
    stroke-dashoffset 480ms cubic-bezier(0.22,1,0.36,1),
    stroke-width 160ms var(--copper-easing);
}
.usage-card__center{position:absolute;inset:20%;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:2px;padding:var(--copper-space-2);text-align:center}
.usage-card__total{font-size:var(--copper-font-size-lg);font-weight:650;font-variant-numeric:tabular-nums}
.usage-card__caption{font-size:var(--copper-font-size-xs);color:var(--copper-text-secondary)}
.usage-card__files{opacity:.72}
.usage-card__scanning{font-size:var(--copper-font-size-sm);color:var(--copper-text-secondary)}
.usage-card__failed{font-size:var(--copper-font-size-sm);color:var(--copper-danger)}
.usage-card__legend{display:grid;gap:var(--copper-space-3);min-width:0}
.usage-card__group{display:grid;gap:2px}
.usage-card__group h3{margin:0 0 2px;font-size:var(--copper-font-size-xs);font-weight:600;letter-spacing:.04em;text-transform:uppercase;color:var(--copper-text-secondary);opacity:.72}
.usage-card__group ul{list-style:none;margin:0;padding:0;display:grid;gap:1px}
.usage-card__group li{display:grid;grid-template-columns:10px minmax(0,1fr) auto minmax(44px,auto);align-items:center;gap:10px;padding:6px 8px;margin:0 -8px;border-radius:var(--copper-radius-sm);min-width:0;font-size:var(--copper-font-size-sm);transition:background-color var(--copper-duration-fast) var(--copper-easing)}
.usage-card__group li.is-hovered{background:var(--copper-hover)}
.usage-card__swatch{width:10px;height:10px;border-radius:3px;flex:0 0 auto}
.usage-card__name{display:grid;gap:1px;min-width:0}
.usage-card__name>span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--copper-text)}
.usage-card__name small{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:var(--copper-font-size-xs);color:var(--copper-text-secondary);opacity:.82}
.usage-card__group li.is-pending .usage-card__name>span{color:var(--copper-text-secondary)}
.usage-card__value{font-variant-numeric:tabular-nums;font-weight:550}
.usage-card__percent{font-size:var(--copper-font-size-xs);color:var(--copper-text-secondary);font-variant-numeric:tabular-nums;text-align:right;min-width:44px}
.usage-card__placeholder{justify-self:end;display:block;width:56px;height:8px;border-radius:var(--copper-radius-full);background:var(--copper-surface-3)}
.usage-card__empty{color:var(--copper-text-secondary);font-size:var(--copper-font-size-sm)}
.usage-value-enter-active,.usage-value-leave-active{transition:opacity 160ms ease,transform 160ms ease}
.usage-value-enter-from{opacity:0;transform:translateY(3px)}
.usage-value-leave-to{opacity:0;transform:translateY(-3px)}
@media(max-width:640px){
  .usage-card{padding:var(--copper-space-4)}
  .usage-card__body{grid-template-columns:1fr;gap:var(--copper-space-4)}
  .usage-card__chart{width:180px}
  .usage-card__group li{grid-template-columns:10px minmax(0,1fr) auto}
  .usage-card__percent{display:none}
}
@media(prefers-reduced-motion:reduce){
  .usage-card__arc,.usage-value-enter-active,.usage-value-leave-active{transition:none}
}
</style>
