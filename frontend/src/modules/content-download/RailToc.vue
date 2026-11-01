<script setup lang="ts">
// 轨道式目录（移植自 Rare UI 的 rail-toc）。
//
// 原实现是 React + motion；这里用 Vue + 原生 SVG 复刻同一套几何与行为：
// - 节点横坐标 = RAIL_X + depth * indent，纵坐标 = 行中心；
// - 节点之间用「先竖、拐弯、再竖」的折线连成一条只向下的路径，虚线打底；
// - 纸飞机沿路径飞行（弹簧插值），飞过的段落变实线，节点被点亮；
// - 到达线 offset 默认 96px，点击条目滚动时以它为准。
//
// 与源码的差异只有动画实现：源码用 motion 的 spring，这里用等价的指数插值 +
// requestAnimationFrame，视觉上是同一个「追过去」的效果。

import { nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

const props = withDefaults(
  defineProps<{
    items: Array<{ id: string; label: string; depth?: number }>;
    /** 滚动容器；为空时监听 window。 */
    scroller?: HTMLElement | null;
    offset?: number;
    indent?: number;
    title?: string;
  }>(),
  { scroller: null, offset: 96, indent: 14, title: "" },
);

const RAIL_X = 5;
const LABEL_GAP = 14;
const PLANE_HOLE = 4;
/** 纸飞机在 24×24 视口里的形状，与源码一致。 */
const PLANE = "M12 2 20.5 21 12 17.5 3.5 21z";

interface Point {
  x: number;
  y: number;
}

const listEl = ref<HTMLElement | null>(null);
const pathEl = ref<SVGPathElement | null>(null);
const rowEls = ref<HTMLElement[]>([]);
const nodes = ref<Point[]>([]);
const pathD = ref("");
const size = ref({ width: 0, height: 0 });
const lengths = ref<number[]>([]);
const total = ref(0);
const reached = ref(1);

/** 纸飞机当前姿态（位置 + 朝向）。 */
const plane = ref({ x: 0, y: 0, angle: 180 });
const filled = ref(0);

let target = 0;
let current = 0;
let raf = 0;
let resizeObserver: ResizeObserver | null = null;

/** 折线：竖直段 + 拐弯段，保证路径只向下（后面按 y 二分查长度才成立）。 */
function buildPath(points: Point[]): string {
  if (points.length === 0) return "";
  const out: Point[] = [{ x: points[0].x, y: 0 }, points[0]];
  for (let i = 1; i < points.length; i += 1) {
    const from = points[i - 1];
    const to = points[i];
    if (from.x !== to.x) {
      const bend = (to.y - from.y) * 0.3;
      out.push({ x: from.x, y: from.y + bend }, { x: to.x, y: to.y - bend });
    }
    out.push(to);
  }
  return out.map((p, i) => (i ? "L" : "M") + p.x + " " + p.y).join(" ");
}

/** 路径只向下 → 给定 y 二分出对应的路径长度。 */
function lengthAtY(path: SVGPathElement, all: number, y: number): number {
  let lo = 0;
  let hi = all;
  for (let i = 0; i < 24; i += 1) {
    const mid = (lo + hi) / 2;
    if (path.getPointAtLength(mid).y < y) lo = mid;
    else hi = mid;
  }
  return hi;
}

/** 朝向：向上翻时掉头朝上。 */
const facing = ref<1 | -1>(1);

function pose(length: number) {
  const path = pathEl.value;
  const all = total.value;
  if (!path || !all) return;
  const at = path.getPointAtLength(length);
  const behind = path.getPointAtLength(Math.max(0, length - 1));
  const ahead = path.getPointAtLength(Math.min(all, length + 1));
  const base = (Math.atan2(ahead.y - behind.y, ahead.x - behind.x) * 180) / Math.PI + 90;
  const angle = base + (facing.value < 0 ? 180 : 0);
  plane.value = { x: at.x, y: at.y, angle };
  filled.value = all > 0 ? length / all : 0;
  reached.value = lengths.value.filter((n) => n <= length + 0.5).length;
}

/** 指数插值追目标：每帧走剩余距离的固定比例，跨帧率也稳定。 */
function tick() {
  raf = 0;
  const diff = target - current;
  if (Math.abs(diff) < 0.35) {
    current = target;
    pose(current);
    return;
  }
  // 往回走超过 2px 才掉头，避免滚动抖动把飞机翻来翻去。
  if (diff < -2 && facing.value !== -1) facing.value = -1;
  else if (diff > 2 && facing.value !== 1) facing.value = 1;
  current += diff * 0.16;
  pose(current);
  raf = requestAnimationFrame(tick);
}

function travelTo(next: number) {
  target = next;
  if (!raf) raf = requestAnimationFrame(tick);
}

function measure() {
  const list = listEl.value;
  if (!list) return;
  const points = props.items.map((item, i) => ({
    x: RAIL_X + (item.depth ?? 0) * props.indent,
    y: (rowEls.value[i]?.offsetTop ?? 0) + (rowEls.value[i]?.offsetHeight ?? 0) / 2,
  }));
  nodes.value = points;
  pathD.value = buildPath(points);
  size.value = {
    width: Math.max(...points.map((p) => p.x), RAIL_X) + RAIL_X + 4,
    height: list.offsetHeight,
  };
  void nextTick(() => {
    const path = pathEl.value;
    if (!path) return;
    total.value = path.getTotalLength();
    lengths.value = points.map((p) => lengthAtY(path, total.value, p.y));
    sync();
  });
}

/** 当前阅读位置 → 目标路径长度（首尾标题都能到达）。 */
function sync() {
  const all = lengths.value;
  if (!all.length) return;
  const el = props.scroller;
  const scrollTop = el ? el.scrollTop : window.scrollY;
  const viewHeight = el ? el.clientHeight : window.innerHeight;
  const scrollHeight = el ? el.scrollHeight : document.documentElement.scrollHeight;
  const originTop = el ? el.getBoundingClientRect().top : 0;

  const remaining = Math.max(0, scrollHeight - viewHeight - scrollTop);
  const line = Math.min(props.offset, scrollTop);
  const anchor = line + Math.max(0, viewHeight - line - remaining);

  const tops = props.items.map((item) => {
    const node = document.getElementById(item.id);
    return node ? node.getBoundingClientRect().top - originTop : Infinity;
  });
  let index = -1;
  for (let i = 0; i < tops.length; i += 1) if (tops[i] <= anchor) index = i;

  let next = all[0];
  if (index >= 0) {
    if (index === all.length - 1) next = all[index];
    else {
      const span = tops[index + 1] - tops[index];
      const progress = span > 0 ? (anchor - tops[index]) / span : 0;
      next = all[index] + Math.min(1, Math.max(0, progress)) * (all[index + 1] - all[index]);
    }
  }
  travelTo(next);
}

function select(item: { id: string }) {
  const el = document.getElementById(item.id);
  if (!el) return;
  const scroller = props.scroller;
  const top = scroller
    ? el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop
    : el.getBoundingClientRect().top + window.scrollY;
  const targetEl = scroller ?? window;
  (targetEl as Window | HTMLElement).scrollTo({ top: top - props.offset, behavior: "smooth" });
}

let detach: (() => void) | null = null;

function bind() {
  detach?.();
  const el = props.scroller;
  const onScroll = () => sync();
  const targetEl: Window | HTMLElement = el ?? window;
  targetEl.addEventListener("scroll", onScroll, { passive: true } as AddEventListenerOptions);
  window.addEventListener("resize", sync);
  detach = () => {
    targetEl.removeEventListener("scroll", onScroll);
    window.removeEventListener("resize", sync);
  };
}

onMounted(() => {
  measure();
  bind();
  resizeObserver = new ResizeObserver(measure);
  if (listEl.value) resizeObserver.observe(listEl.value);
  void document.fonts?.ready.then(measure).catch(() => {});
});

watch(
  () => props.items,
  () => {
    measure();
  },
  { deep: true },
);

watch(
  () => props.scroller,
  () => {
    measure();
    bind();
  },
);

onBeforeUnmount(() => {
  detach?.();
  resizeObserver?.disconnect();
  if (raf) cancelAnimationFrame(raf);
});
</script>

<template>
  <nav class="rail-toc" :aria-label="title">
    <p class="rail-toc__title">
      <svg viewBox="0 0 16 16" class="rail-toc__title-icon" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true">
        <path d="M2 4h12M2 8h8M2 12h10" />
      </svg>
      {{ title }}
    </p>

    <ul ref="listEl" class="rail-toc__list">
      <svg
        v-if="pathD"
        class="rail-toc__svg"
        :width="size.width"
        :height="size.height"
        aria-hidden="true"
      >
        <path
          ref="pathEl"
          :d="pathD"
          fill="none"
          stroke-width="1.25"
          stroke-dasharray="2 5"
          stroke-linecap="round"
          class="rail-toc__rail"
        />
        <path
          :d="pathD"
          fill="none"
          stroke-width="1.25"
          stroke-linejoin="round"
          class="rail-toc__rail-filled"
          :style="{ strokeDasharray: total + ' ' + total, strokeDashoffset: total * (1 - filled) }"
        />
        <circle
          v-for="(node, index) in nodes"
          :key="'n' + index"
          :cx="node.x"
          :cy="node.y"
          :r="index < reached ? 3 : 3.25"
          :class="['rail-toc__node', { 'rail-toc__node--on': index < reached }]"
        />
        <!-- 纸飞机：用遮罩在导轨与节点上挖一个洞，再把飞机盖上去 -->
        <g :transform="'translate(' + (plane.x - 9) + ',' + (plane.y - 9) + ') rotate(' + plane.angle + ' 9 9) scale(0.75)'">
          <circle :cx="12" :cy="12" :r="PLANE_HOLE / 0.75" class="rail-toc__hole" />
          <path :d="PLANE" class="rail-toc__plane" />
        </g>
      </svg>

      <li
        v-for="(item, index) in items"
        :key="item.id"
        :ref="(el) => { rowEls[index] = el as HTMLElement }"
        class="rail-toc__row"
      >
        <button
          type="button"
          class="rail-toc__item"
          :class="{ 'is-active': index === reached - 1, 'is-passed': index < reached }"
          :style="{ paddingLeft: RAIL_X + (item.depth ?? 0) * indent + LABEL_GAP + 'px' }"
          @click="select(item)"
        >
          {{ item.label }}
        </button>
      </li>
    </ul>
  </nav>
</template>

<style scoped>
.rail-toc {
  --rail: color-mix(in srgb, var(--copper-text) 45%, var(--copper-bg));
  width: max-content;
  max-width: 100%;
  font-size: var(--copper-font-size-sm);
}

.rail-toc__title {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  margin: 0 0 var(--copper-space-3);
  color: color-mix(in srgb, var(--copper-text) 70%, transparent);
  font-weight: 500;
}

.rail-toc__title-icon {
  width: 14px;
  height: 14px;
}

.rail-toc__list {
  position: relative;
  margin: 0;
  padding: 0;
  list-style: none;
}

.rail-toc__svg {
  position: absolute;
  left: 0;
  top: 0;
  overflow: visible;
  pointer-events: none;
}

.rail-toc__rail {
  stroke: color-mix(in srgb, var(--copper-text) 25%, transparent);
}

.rail-toc__rail-filled {
  stroke: var(--rail);
}

.rail-toc__node {
  fill: var(--copper-bg);
  stroke: color-mix(in srgb, var(--copper-text) 35%, transparent);
  stroke-width: 1.25;
}

.rail-toc__node--on {
  fill: var(--rail);
  stroke: var(--rail);
}

/* 挖洞：用页面底色盖住导轨与节点，飞机才像是「骑」在轨道上 */
.rail-toc__hole {
  fill: var(--copper-bg);
}

.rail-toc__plane {
  fill: var(--copper-text);
}

.rail-toc__row {
  position: relative;
}

.rail-toc__item {
  display: block;
  width: 100%;
  border: none;
  background: transparent;
  color: color-mix(in srgb, var(--copper-text) 55%, transparent);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  line-height: 1.45;
  text-align: left;
  padding-top: 5px;
  padding-bottom: 5px;
  padding-right: 6px;
  cursor: pointer;
  transition: color var(--copper-duration-fast) var(--copper-easing);
}

.rail-toc__item:hover,
.rail-toc__item.is-passed {
  color: var(--copper-text);
}

.rail-toc__item.is-active {
  color: var(--copper-text);
  font-weight: 600;
}
</style>
