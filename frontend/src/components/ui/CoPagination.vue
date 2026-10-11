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

import { computed } from "vue";
import { ChevronRight } from "@lucide/vue";

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
  { loading: false },
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

function go(target: number) {
  if (props.loading) return;
  // 越界与重复点击直接吞掉：调用方的 page 可能还没被新响应改回来，
  // 连点两下同一个页码会触发两次拉取。
  if (target < 0 || target >= props.totalPages || target === props.page) return;
  emit("change", target);
}
</script>

<template>
  <nav v-if="visible" class="co-pager" :aria-label="labels.nav">
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

    <!-- 桌面形态：面包屑页码序列 -->
    <div class="co-pager__crumbs">
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

html[data-form-factor="mobile"] .co-pager__compact-state {
  display: none;
}
</style>
