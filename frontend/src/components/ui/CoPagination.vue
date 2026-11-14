<script setup lang="ts">
// 分页：面包屑式页码条，全站唯一的翻页 UI。
//
// 形态由需求钉死：
// - 页码读起来是**面包屑**而不是一排等权按钮：相邻页码之间用 chevron 分隔，读者一眼
//   就知道这是一条「有序序列」，而不是一堆互不相关的跳转按钮；
// - 必须**直接显示已知页数**，并且能跳到任意一页（旧实现只能一路点「下一页」，看第 20 页
//   要点 19 次）；
// - `totalPages` 是「已探索到的页数」而非权威总数：抓取型接口不提供总数，未翻到的页
//   本来就未知，编一个假数字不如如实呈现（详见 content-download/listStore.ts）。
//   调用方也可以直接传权威总数，组件对两种语义一视同仁。
//
// 窄屏降级：`html[data-form-factor="mobile"]` 下隐藏整条页码序列，只留
// 「上一页 / 第 X / N 页 / 下一页」——一屏 7 个页码位在手机上会把整个页脚挤成两行。

import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { ChevronRight } from "@lucide/vue";

import CoNumberWheel from "./CoNumberWheel.vue";

/** 面包屑项：页码，或省略号（`-1` 表示「还有更多页」）。 */
type PageItem = number | "gap";

const props = withDefaults(
  defineProps<{
    /** 当前页，**0 基**（与后端 query.page 一致）。 */
    page: number;
    /**
     * 已知页数（1 基意义下的页数下界），**不是**权威总页数。
     *
     * 抓取型接口不提供总数（见 content-download/listStore.ts 对三条路径的对照），
     * 所以翻页 UI 只能呈现「已探索到的页数」：还有下一页时至少算当前页 +2，
     * 没有下一页时当前页就是末页。末页数字必然准确，往前翻时单调增长。
     * 传真实总数进来同样成立，只是那时它就是真总页数。
     */
    totalPages: number;
    /** 翻页中：禁用所有跳转，但不隐藏控件（避免布局跳动）。 */
    loading?: boolean;
    /**
     * 紧凑形态：隐藏面包屑页码序列，只保留「上一页 / 下一页」。
     * 用于宽度有限的容器（右栏、抽屉），与移动端降级共用同一套结构。
     */
    compact?: boolean;
    /**
     * 形态：
     * - `crumb`（默认）：面包屑页码序列，全宽页脚用；
     * - `circle`：全圆形按钮（上一页 / 页码 / 下一页），右栏或卡片头部用。
     */
    shape?: "crumb" | "circle";
    /**
     * 待定页码（**1 基**），只影响当前页那一位的显示：传值进来时那一格用
     * 滚轮数字跟着变，但不改页码序列的排版、也不触发翻页。
     *
     * 与 CoPageScrubber 配合：拖动扫页时把待定值喂到这里，视线只需要盯一个数字；
     * 松手由调用方真正翻页，`preview` 复位。
     */
    preview?: number | null;
    /**
     * 文案由调用方按自己的 i18n 命名空间注入。
     * 组件本身不引入 i18n：它是内核控件，不该知道「内容下载」这种模块概念。
     */
    labels: {
      /** 「第 {current} / {total} 页」 */
      summary: (current: number, total: number) => string;
      prev: string;
      next: string;
      /** 无障碍：导航区名称 */
      nav: string;
      /** 无障碍：跳转按钮读法，形如「第 5 页」 */
      goto: (page: number) => string;
    };
  }>(),
  { loading: false, compact: false, shape: "crumb", preview: null },
);

const emit = defineEmits<{ (e: "change", page: number): void }>();

const current = computed(() => props.page + 1);

/** 是否值得渲染整条：只有一页且无处可去时，翻页毫无意义，直接不出现。 */
const visible = computed(() => props.totalPages > 1);

const canPrev = computed(() => props.page > 0 && !props.loading);
const canNext = computed(() => current.value < props.totalPages && !props.loading);

/**
 * 页码序列：首页 + 末页常驻，当前页左右各一，其余折叠成省略号。
 * 长度上限 7（1 … 4 5 6 … 23 ），再多就会撑破页脚并让两端权重失衡。
 */
const items = computed<PageItem[]>(() => {
  const last = props.totalPages;
  const cur = current.value;
  const out: PageItem[] = [];
  if (last >= 1) out.push(1);
  const from = Math.max(2, cur - 1);
  const to = Math.min(last - 1, cur + 1);
  if (from > 2) out.push("gap");
  for (let n = from; n <= to; n += 1) out.push(n);
  if (to < last - 1) out.push("gap");
  if (last >= 2) out.push(last);
  return out;
});

/** 圆形按钮的最小宽度与间距，用于按容器宽度换算能放几个页码。 */
const CIRCLE_MIN_WIDTH = 30;
const CIRCLE_GAP = 4;
/** 左右两个翻页按钮固定占位。 */
const CIRCLE_ARROWS = 2;
/** 页码槽位上限：再多就失去「一眼看清位置」的意义。 */
const CIRCLE_MAX_SLOTS = 11;

const circlesRef = ref<HTMLElement | null>(null);
/**
 * 当前宽度下能放下的页码槽位数（含省略号位）。
 *
 * 右栏只有 500px 上下时给 11 位，窄容器退到 3~5 位；上限见 CIRCLE_MAX_SLOTS。
 */
const circleCapacity = ref(5);
let circlesObserver: ResizeObserver | null = null;

function measureCircleCapacity(width: number) {
  if (width <= 0) return;
  const fit =
    Math.floor((width + CIRCLE_GAP) / (CIRCLE_MIN_WIDTH + CIRCLE_GAP)) - CIRCLE_ARROWS;
  circleCapacity.value = Math.max(3, Math.min(CIRCLE_MAX_SLOTS, fit));
}

function attachCirclesObserver(el: HTMLElement | null) {
  circlesObserver?.disconnect();
  circlesObserver = null;
  if (!el) return;
  measureCircleCapacity(el.getBoundingClientRect().width);
  // 窗口最大化 / 分栏变宽都会改容器宽度，槽位必须跟着变，否则右栏会留一大片空白。
  if (typeof ResizeObserver === "undefined") return;
  circlesObserver = new ResizeObserver((entries) => {
    const entry = entries[0];
    if (entry) measureCircleCapacity(entry.contentRect.width);
  });
  circlesObserver.observe(el);
}

// 元素是 v-if 渲染的（只有一页 / 面包屑形态时不存在），只能跟着 ref 的落地时机挂观察器。
watch(circlesRef, (el) => attachCirclesObserver(el), { flush: "post" });
onMounted(() => attachCirclesObserver(circlesRef.value));

onBeforeUnmount(() => {
  circlesObserver?.disconnect();
  circlesObserver = null;
});

/** 以当前页为中心、宽度为 `width` 的连续页码窗口，首末页常驻，两侧缺口折叠成省略号。 */
function windowItems(last: number, cur: number, width: number): PageItem[] {
  let start = Math.max(2, cur - Math.floor((width - 1) / 2));
  let end = Math.min(last - 1, start + width - 1);
  start = Math.max(2, end - width + 1);
  const out: PageItem[] = [1];
  if (start > 2) out.push("gap");
  for (let n = start; n <= end; n += 1) out.push(n);
  if (end < last - 1) out.push("gap");
  out.push(last);
  return out;
}

/**
 * 圆形形态的页码序列：**槽位数随容器宽度自适应**。
 *
 * 之前固定 5 个槽位（1 … 当前 … 末），窗口最大化后右栏变宽仍只显示三位数字，
 * 中间一大片空白。现在先量容器宽度算出能放 C 个槽位，再把「首末页 + 两条省略号」
 * 之外的槽位都给当前页附近的连续窗口；窄容器自动退化成 1 … N。
 */
const circleItems = computed<PageItem[]>(() => {
  const last = props.totalPages;
  const cur = current.value;
  const capacity = circleCapacity.value;
  if (last <= capacity) {
    return Array.from({ length: Math.max(last, 1) }, (_, i) => i + 1);
  }
  if (capacity <= 4) return [1, "gap", last];
  // 首末页 + 两条省略号占 4 个槽位，其余给窗口；窗口从宽到窄回落，
  // 保证「凑不出省略号」的边界页附近也能填满可用宽度。
  let width = Math.max(1, capacity - 4);
  for (;;) {
    const items = windowItems(last, cur, width);
    if (items.length <= capacity || width <= 1) return items;
    width -= 1;
  }
});

function go(target: number) {
  if (props.loading) return;
  // 越界与重复点击直接吞掉：调用方的 page 可能还没被新响应改回来，
  // 连点两下同一个页码会触发两次拉取。
  if (target < 0 || target >= props.totalPages || target === props.page) return;
  emit("change", target);
}
</script>

<template>
  <nav
    v-if="visible"
    :class="['co-pager', { 'co-pager--compact': compact }]"
    :aria-label="labels.nav"
  >
    <!-- 汇总行：页数 -->
    <p class="co-pager__summary">
      <span class="co-pager__summary-page">{{ labels.summary(current, totalPages) }}</span>
    </p>

    <!-- 窄屏形态：页码序列塞不下，只留前后翻页 + 当前页 -->
    <div class="co-pager__compact">
      <button
        class="co-pager__step"
        :disabled="!canPrev"
        @click="go(page - 1)"
      >
        <ChevronRight :size="15" class="co-pager__step-icon co-pager__step-icon--back" />
        {{ labels.prev }}
      </button>
      <span class="co-pager__compact-state">{{ labels.summary(current, totalPages) }}</span>
      <button
        class="co-pager__step"
        :disabled="!canNext"
        @click="go(page + 1)"
      >
        {{ labels.next }}
        <ChevronRight :size="15" class="co-pager__step-icon" />
      </button>
    </div>

    <!-- 圆形形态：右栏 / 卡片头部用 -->
    <div v-if="shape === 'circle'" ref="circlesRef" class="co-pager__circles">
      <button
        class="co-pager__circle co-pager__circle--step"
        :disabled="!canPrev"
        :aria-label="labels.prev"
        @click="go(page - 1)"
      >
        <ChevronRight :size="15" class="co-pager__step-icon co-pager__step-icon--back" />
      </button>

      <template v-for="(item, index) in circleItems" :key="`c-${item}-${index}`">
        <span v-if="item === 'gap'" class="co-pager__circle-gap" aria-hidden="true">…</span>
        <button
          v-else
          class="co-pager__circle"
          :class="{ 'co-pager__circle--current': item === current }"
          :aria-current="item === current && preview == null ? 'page' : undefined"
          :aria-label="labels.goto(preview != null && item === current ? preview : item)"
          :disabled="loading"
          @click="go(item - 1)"
        >
          <span
            v-if="loading && item === current"
            class="co-pager__num-spinner"
            aria-hidden="true"
          />
          <CoNumberWheel
            v-else-if="preview != null && item === current"
            class="co-pager__wheel"
            :value="preview"
            :duration="0.28"
          />
          <template v-else>{{ item }}</template>
        </button>
      </template>

      <button
        class="co-pager__circle co-pager__circle--step"
        :disabled="!canNext"
        :aria-label="labels.next"
        @click="go(page + 1)"
      >
        <ChevronRight :size="15" class="co-pager__step-icon" />
      </button>
    </div>

    <!-- 桌面形态：面包屑页码序列 -->
    <div v-else class="co-pager__crumbs">
      <button
        class="co-pager__step"
        :disabled="!canPrev"
        @click="go(0)"
      >
        <ChevronRight :size="15" class="co-pager__step-icon co-pager__step-icon--back" />
        {{ labels.prev }}
      </button>

      <template v-for="(item, index) in items" :key="`${item}-${index}`">
        <ChevronRight :size="13" class="co-pager__sep" aria-hidden="true" />
        <span v-if="item === 'gap'" class="co-pager__gap" aria-hidden="true">…</span>
        <button
          v-else
          class="co-pager__num"
          :class="{ 'co-pager__num--current': item === current }"
          :aria-current="item === current ? 'page' : undefined"
          :aria-label="labels.goto(item)"
          :disabled="loading"
          @click="go(item - 1)"
        >
          <!-- 翻页中把**当前页码位**换成加载圈，而不是整块替换：
               整块替换会让页脚高度与内容跳动，用户的视线要重新找一次。 -->
          <span
            v-if="loading && item === current"
            class="co-pager__num-spinner"
            aria-hidden="true"
          />
          <template v-else>{{ item }}</template>
        </button>
      </template>

      <ChevronRight :size="13" class="co-pager__sep" aria-hidden="true" />
      <button
        class="co-pager__step"
        :disabled="!canNext"
        @click="go(totalPages - 1)"
      >
        {{ labels.next }}
        <ChevronRight :size="15" class="co-pager__step-icon" />
      </button>
    </div>
  </nav>
</template>

<style scoped>
.co-pager {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-2);
  padding-top: var(--copper-space-4);
}

/* 汇总行 */
.co-pager__summary {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-secondary);
}

.co-pager__summary-page {
  font-weight: 600;
  color: var(--copper-text);
}

/* 窄屏形态：默认隐藏，只在 mobile 表单因子里出现 */
.co-pager__compact {
  display: none;
  align-items: center;
  gap: var(--copper-space-3);
}

.co-pager__compact-state {
  min-width: 92px;
  text-align: center;
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
  color: var(--copper-text);
}

/* 圆形形态：无描边，靠底色阶差与选中态区分。 */
.co-pager__circles {
  display: flex;
  align-items: center;
  /* 固定一行：不换行，靠 space-between 铺满左右。 */
  flex-wrap: nowrap;
  justify-content: space-between;
  gap: 4px;
  width: 100%;
}

.co-pager__circle {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 30px;
  height: 30px;
  padding: 0 7px;
  border: none;
  border-radius: 50%;
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  font-variant-numeric: tabular-nums;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.co-pager__circle:hover:not(:disabled) {
  background: var(--copper-surface-3);
  color: var(--copper-text);
}

.co-pager__circle--current {
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
  font-weight: 600;
}

.co-pager__circle:disabled {
  opacity: 0.45;
  cursor: default;
}

.co-pager__circle--step {
  color: var(--copper-text);
}

.co-pager__circle-gap {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
}

/* 待定页码用滚轮数字，同字号、同色，只是会滚。 */
.co-pager__wheel {
  display: inline-flex;
  line-height: 1;
}

/* 面包屑主体 */
.co-pager__crumbs {
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 3px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
}

/* 分隔 chevron：颜色压到最弱，它是秩序符号不是内容。 */
.co-pager__sep {
  flex-shrink: 0;
  color: var(--copper-text-disabled);
}

.co-pager__gap {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 22px;
  height: var(--copper-control-h-sm);
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
}

/* 前后翻页按钮 */
.co-pager__step {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-1);
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-full);
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  font-family: inherit;
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.co-pager__step:hover:not(:disabled) {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.co-pager__step:disabled {
  opacity: 0.4;
  cursor: default;
}

.co-pager__step-icon--back {
  transform: rotate(180deg);
}

/* 页码按钮 */
.co-pager__num {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 26px;
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-2);
  border: none;
  border-radius: var(--copper-radius-full);
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  font-family: inherit;
  font-variant-numeric: tabular-nums;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.co-pager__num:hover:not(:disabled):not(.co-pager__num--current) {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.co-pager__num:disabled {
  cursor: default;
}

/* 当前页：唯一有底色的页码位。它是面包屑上的「定位点」，
 * 用强调色实底而不是加粗——加粗在 12px 下几乎看不出差别。 */
.co-pager__num--current {
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
  font-weight: 600;
  cursor: default;
}

.co-pager__num-spinner {
  width: 13px;
  height: 13px;
  border: 2px solid currentColor;
  border-right-color: transparent;
  border-radius: 50%;
  animation: co-pager-spin 0.7s linear infinite;
}

@keyframes co-pager-spin {
  to {
    transform: rotate(360deg);
  }
}

/* 窄屏：隐藏面包屑序列，退化为前后翻页 + 当前页。 */
html[data-form-factor="mobile"] .co-pager__crumbs {
  display: none;
}

html[data-form-factor="mobile"] .co-pager__compact {
  display: flex;
}

/* 显式紧凑形态：与移动端同一套降级，但不依赖表单因子。 */
.co-pager--compact .co-pager__crumbs {
  display: none;
}

.co-pager--compact .co-pager__compact {
  display: flex;
}

/* 汇总行已经写了页数，紧凑形态的中间状态文案是重复信息。 */
.co-pager--compact .co-pager__compact-state {
  display: none;
}

html[data-form-factor="mobile"] .co-pager__compact-state {
  display: none;
}
</style>
