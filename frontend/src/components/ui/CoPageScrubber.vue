<script setup lang="ts">
// 跳页刻度尺：横拖整行扫页，松手才真正翻页。
//
// 形态照搬 rare-ui animated-counter 的 demo（app/components/(docs)/animatedcounter/demo.tsx）：
// 一排等宽刻度铺满左右（justify-between），已经过的刻度高 20px，未到的 14px，
// 当前那一根换成 3×28px 的强调色刻度；刻度宽 2px，恰好让「当前」这根视觉上比别的粗一档。
// 刻意不做滑块 + 轨道的传统 slider 观感——用户要的是这把尺子。
//
// 交互约定（需求钉死）：
// - 拖动期间只发 `preview`，**不发请求**；指针抬起才发 `commit`；
// - Esc / pointercancel 直接作废，回到当前页，不发请求；
// - 点任意位置 = 落到该处；滚轮一格一页；键盘方向键一页、PageUp/Down 十页、
//   Home/End 首末、直接打数字则按输入的值跳（保底，回到旧输入框的手感）。
// - 页数 <= 1 时整个控件不渲染；loading 时禁止拖动，避免请求竞赛。

import { computed, onBeforeUnmount, onMounted, ref } from "vue";

const props = withDefaults(
  defineProps<{
    /** 当前页，**0 基**（与后端 query.page 一致）。 */
    page: number;
    /** 总页数（1 基）。 */
    totalPages: number;
    /** 加载中：禁用拖动，但保持形态不闪烁。 */
    loading?: boolean;
    /** 文案由调用方按自己的 i18n 命名空间注入。 */
    labels: {
      /** 无障碍标签 / 悬浮提示：「跳至」 */
      jump: string;
      /** 无障碍读法，形如「第 5 页」 */
      goto: (page: number) => string;
    };
  }>(),
  { loading: false },
);

const emit = defineEmits<{
  /** 拖动中的待定页（1 基）；结束或作废时为 null。 */
  (e: "preview", value: number | null): void;
  /** 松手 / 键盘落定后的目标页（1 基）。 */
  (e: "commit", value: number): void;
}>();

const root = ref<HTMLElement | null>(null);
const dragging = ref(false);
/** 拖动中指针的连续位置（0~1）。刻度只跟着它走，不参与取值。 */
const dragRatio = ref(0);
const pending = ref<number | null>(null);
/** 实测宽度 -> 刻度根数，保证刻度间距恒为 ~10px（原 demo 的 41 根是 384px 宽时的密度）。 */
const tickCount = ref(41);

const current = computed(() => props.page + 1);
const total = computed(() => Math.max(props.totalPages, 1));
const disabled = computed(() => props.loading || props.totalPages <= 1);
const visible = computed(() => props.totalPages > 1);

/** 当前刻度位置（0~1）：拖动中跟指针，否则跟真实页码。 */
const ratio = computed(() => {
  if (dragging.value) return dragRatio.value;
  if (total.value <= 1) return 0;
  return (current.value - 1) / (total.value - 1);
});

/** 强调色刻度落点：吸附到最近一根刻度上（原 demo 的做法，保证「当前」压在刻度位）。 */
const marker = computed(() => Math.round(ratio.value * (tickCount.value - 1)));

const ticks = computed(() =>
  Array.from({ length: tickCount.value }, (_, i) => ({
    key: i,
    kind: i === marker.value ? "marker" : i < marker.value ? "passed" : "future",
  })),
);

function clampPage(n: number) {
  return Math.min(Math.max(Math.round(n), 1), total.value);
}

function pageAt(clientX: number) {
  const el = root.value;
  if (!el) return current.value;
  const rect = el.getBoundingClientRect();
  if (rect.width <= 0) return current.value;
  const r = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
  dragRatio.value = r;
  return clampPage(1 + r * (total.value - 1));
}

function setPreview(v: number) {
  if (pending.value === v) return;
  pending.value = v;
  emit("preview", v);
}

function clearPreview() {
  if (pending.value === null) return;
  pending.value = null;
  emit("preview", null);
}

function onDown(e: PointerEvent) {
  if (disabled.value) return;
  if (e.pointerType === "mouse" && e.button !== 0) return;
  e.preventDefault();
  dragging.value = true;
  const el = e.currentTarget as HTMLElement;
  el.setPointerCapture?.(e.pointerId);
  // 不用 focus()：鼠标按下拿到焦点会让 :focus-visible 亮起一圈描边，
  // 拖动过程中整把尺子被框住很难看。取消拖动走 window 级的 Esc，不依赖焦点。
  window.addEventListener("keydown", onWindowKeydown);
  setPreview(pageAt(e.clientX));
}

function onMove(e: PointerEvent) {
  if (!dragging.value) return;
  setPreview(pageAt(e.clientX));
}

/** 拖动中按 Esc 作废（不依赖焦点，指针已经挂在尺子上）。 */
function onWindowKeydown(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  e.preventDefault();
  onCancel();
}

function stopTracking() {
  window.removeEventListener("keydown", onWindowKeydown);
}

function onUp() {
  if (!dragging.value) return;
  const target = pending.value;
  dragging.value = false;
  stopTracking();
  clearPreview();
  if (target !== null && target !== current.value) emit("commit", target);
}

function onCancel() {
  if (!dragging.value) return;
  dragging.value = false;
  stopTracking();
  clearPreview();
}

function commitTo(v: number) {
  if (disabled.value) return;
  const target = clampPage(v);
  if (target !== current.value) emit("commit", target);
}

function onWheel(e: WheelEvent) {
  if (disabled.value) return;
  commitTo(current.value + (e.deltaY > 0 ? 1 : -1));
}

/** 直接打数字：800ms 内的连续按键拼成一个数，超时或跳页后清零。 */
let typed = "";
let typedTimer: ReturnType<typeof setTimeout> | null = null;

function resetTyped() {
  typed = "";
  if (typedTimer) clearTimeout(typedTimer);
  typedTimer = null;
}

function onKeydown(e: KeyboardEvent) {
  if (disabled.value) return;
  switch (e.key) {
    case "ArrowRight":
    case "ArrowUp":
      commitTo(current.value + 1);
      break;
    case "ArrowLeft":
    case "ArrowDown":
      commitTo(current.value - 1);
      break;
    case "PageUp":
      commitTo(current.value + 10);
      break;
    case "PageDown":
      commitTo(current.value - 10);
      break;
    case "Home":
      commitTo(1);
      break;
    case "End":
      commitTo(total.value);
      break;
    case "Escape":
      onCancel();
      return;
    default: {
      if (!/^[0-9]$/.test(e.key)) return;
      typed = (typed + e.key).slice(-6);
      if (typedTimer) clearTimeout(typedTimer);
      typedTimer = setTimeout(resetTyped, 800);
      commitTo(Number(typed));
      return;
    }
  }
  resetTyped();
}

/** 尺子密度跟宽度走：宽度每 10px 一根，夹在 9~41 根（原 demo 的上下限）。 */
let observer: ResizeObserver | null = null;

function measure() {
  const el = root.value;
  if (!el) return;
  const w = el.getBoundingClientRect().width;
  if (w <= 0) return;
  tickCount.value = Math.min(41, Math.max(9, Math.round(w / 10)));
}

onMounted(() => {
  measure();
  if (typeof ResizeObserver === "undefined" || !root.value) return;
  observer = new ResizeObserver(measure);
  observer.observe(root.value);
});

onBeforeUnmount(() => {
  observer?.disconnect();
  observer = null;
  stopTracking();
  if (typedTimer) clearTimeout(typedTimer);
});
</script>

<template>
  <div
    v-if="visible"
    ref="root"
    class="co-ruler"
    :class="{ 'co-ruler--dragging': dragging, 'co-ruler--loading': loading }"
    role="slider"
    tabindex="0"
    :title="labels.jump"
    :aria-label="labels.jump"
    :aria-valuemin="1"
    :aria-valuemax="totalPages"
    :aria-valuenow="pending ?? current"
    :aria-valuetext="labels.goto(pending ?? current)"
    :aria-disabled="disabled || undefined"
    @pointerdown="onDown"
    @pointermove="onMove"
    @pointerup="onUp"
    @pointercancel="onCancel"
    @keydown="onKeydown"
    @wheel.prevent="onWheel"
  >
    <span
      v-for="tick in ticks"
      :key="tick.key"
      class="co-ruler__tick"
      :class="`co-ruler__tick--${tick.kind}`"
      aria-hidden="true"
    />
  </div>
</template>

<style scoped>
/* 尺子本体：32px 高，刻度底部对齐（原 demo 的 flex h-8 items-end）。 */
.co-ruler {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  width: 100%;
  height: 32px;
  /* 左右各留 1px：当前刻度比普通刻度宽 1px，贴边时会被裁掉半个像素。 */
  padding: 0 1px;
  margin-top: var(--copper-space-1);
  border-radius: var(--copper-radius-sm);
  outline: none;
  cursor: grab;
  touch-action: none;
  user-select: none;
}

.co-ruler--dragging {
  cursor: grabbing;
}

.co-ruler:focus-visible {
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--copper-accent) 45%, transparent);
}

/* 刻度：2px 宽、两端圆头；高度与颜色按「已过 / 未到」切换，过渡让尺子有呼吸感。 */
.co-ruler__tick {
  display: block;
  flex: 0 0 auto;
  width: 2px;
  border-radius: var(--copper-radius-full);
  transition:
    height 200ms var(--copper-easing),
    background-color 200ms var(--copper-easing);
}

.co-ruler__tick--passed {
  height: 20px;
  background: color-mix(in srgb, var(--copper-text) 78%, transparent);
}

.co-ruler__tick--future {
  height: 14px;
  background: color-mix(in srgb, var(--copper-text) 22%, transparent);
}

/* 当前那根：更宽更高、强调色实底。左右各让出 0.5px 抵消宽度差，不把邻居挤开。 */
.co-ruler__tick--marker {
  width: 3px;
  height: 28px;
  margin: 0 -0.5px;
  background: var(--copper-accent);
}

/* 加载中：只降饱和，不隐藏（隐藏会让页脚高度跳动）。 */
.co-ruler--loading {
  cursor: progress;
}

/* 减动效：高度直接切换，不做过渡。 */
@media (prefers-reduced-motion: reduce) {
  .co-ruler__tick {
    transition: none;
  }
}
</style>
