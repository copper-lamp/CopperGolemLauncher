<script setup lang="ts">
// 标签条原语（内核通用组件）：只渲染标签条，不含面板。
//
// 视觉母题取自浏览器标签页 —— 选中项与相邻面板同色、被咬合那一侧的边框断开，
// 形成一条无缝接缝。实现方式：选中项的 `::after` 用面板同色覆盖面板那条 1px 边框，
// 不使用负边距，因此切换标签时零布局位移。
//
// 方向：
// - `vertical`：纵向条带，咬合右边（面板在右侧）；
// - `horizontal`：横向条带，咬合下边（面板在下方）。
//
// 调用方契约（重要）：
// 1. 面板 `background` 必须是 `var(--copper-surface)`，被咬合的那条边为
//    `1px solid var(--copper-border)`；
// 2. 条带与被咬合的面板之间不能有间隙（父容器不要在此处加 gap / margin），
//    否则接缝会落在空隙里；
// 3. 条带自身不要设置滚动（需要滚动请在条带外层套 overflow 容器）。
//
// 选中指示条（3px 强调色）用 `transform` 位移，切换标签时只重绘、不重排；
// 复杂标签内容（图片、多行）用 `#item` 插槽替换默认的 图标 + 文本。

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { Component } from "vue";

const props = withDefaults(
  defineProps<{
    modelValue: string;
    /** 标签项；`icon` 为 @lucide/vue 图标组件。 */
    items: Array<{ value: string; label: string; icon?: Component }>;
    direction?: "vertical" | "horizontal";
  }>(),
  { direction: "horizontal" },
);

const emit = defineEmits<{ "update:modelValue": [value: string] }>();

/** 指示条粗细。 */
const BAR_SIZE = 3;
/** 指示条沿条带方向的内缩，避免顶到圆角。 */
const BAR_INSET = 6;

const listEl = ref<HTMLElement | null>(null);
const bar = ref({ x: 0, y: 0, w: 0, h: 0, visible: false });

const activeIndex = computed(() =>
  props.items.findIndex((item) => item.value === props.modelValue),
);

/**
 * 用包围盒差值定位指示条（容器无边框，故等效于其 padding 盒原点，
 * 与绝对定位的 `left: 0; top: 0` 基准一致）。
 */
async function measure() {
  await nextTick();
  const list = listEl.value;
  const index = activeIndex.value;
  const tab =
    list && index >= 0
      ? list.querySelectorAll<HTMLElement>("[data-co-tab]")[index]
      : null;
  if (!list || !tab) {
    bar.value = { ...bar.value, visible: false };
    return;
  }
  const listRect = list.getBoundingClientRect();
  const tabRect = tab.getBoundingClientRect();
  const x = tabRect.left - listRect.left;
  const y = tabRect.top - listRect.top;
  bar.value =
    props.direction === "vertical"
      ? {
          x,
          y: y + BAR_INSET,
          w: BAR_SIZE,
          h: Math.max(tabRect.height - BAR_INSET * 2, BAR_SIZE),
          visible: true,
        }
      : {
          x: x + BAR_INSET,
          y,
          w: Math.max(tabRect.width - BAR_INSET * 2, BAR_SIZE),
          h: BAR_SIZE,
          visible: true,
        };
}

let observer: ResizeObserver | null = null;

onMounted(() => {
  void measure();
  if (listEl.value && typeof ResizeObserver !== "undefined") {
    observer = new ResizeObserver(() => void measure());
    observer.observe(listEl.value);
  }
});

onBeforeUnmount(() => {
  observer?.disconnect();
  observer = null;
});

watch(
  () => [props.modelValue, props.items.length, props.direction],
  () => void measure(),
);

function select(value: string) {
  if (value === props.modelValue) return;
  emit("update:modelValue", value);
}
</script>

<template>
  <div
    ref="listEl"
    :class="['co-tabs', `co-tabs--${direction}`]"
    role="tablist"
  >
    <button
      v-for="item in items"
      :key="item.value"
      data-co-tab
      type="button"
      role="tab"
      :aria-selected="item.value === modelValue"
      :class="['co-tabs__tab', { 'co-tabs__tab--active': item.value === modelValue }]"
      @click="select(item.value)"
    >
      <slot name="item" :item="item" :active="item.value === modelValue">
        <component :is="item.icon" v-if="item.icon" :size="15" />
        <span class="co-tabs__label">{{ item.label }}</span>
      </slot>
    </button>

    <span
      :class="['co-tabs__indicator', { 'co-tabs__indicator--hidden': !bar.visible }]"
      :style="{
        width: `${bar.w}px`,
        height: `${bar.h}px`,
        transform: `translate(${bar.x}px, ${bar.y}px)`,
      }"
    />
  </div>
</template>

<style scoped>
.co-tabs {
  position: relative;
  z-index: 1; /* 抬升条带，使选中项能压住相邻面板的边框 */
  border: none;
}

.co-tabs--vertical {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.co-tabs--horizontal {
  display: flex;
  flex-direction: row;
  gap: 2px;
}

.co-tabs__tab {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  border: 1px solid transparent;
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.co-tabs--vertical .co-tabs__tab {
  width: 100%;
  padding: var(--copper-space-2) var(--copper-space-3);
  border-radius: var(--copper-radius-md);
}

.co-tabs--horizontal .co-tabs__tab {
  padding: var(--copper-space-2) var(--copper-space-4);
  border-radius: var(--copper-radius-md) var(--copper-radius-md) 0 0;
}

.co-tabs__tab:hover:not(.co-tabs__tab--active) {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.co-tabs__tab--active {
  position: relative;
  background: var(--copper-surface);
  color: var(--copper-text);
  font-weight: 500;
}

/* 纵向：咬合右边 —— 右边框取面板同色，::after 覆盖面板的 1px 左边框。 */
.co-tabs--vertical .co-tabs__tab--active {
  border-color: var(--copper-border);
  border-right-color: var(--copper-surface);
  border-top-right-radius: 0;
  border-bottom-right-radius: 0;
}

.co-tabs--vertical .co-tabs__tab--active::after {
  content: "";
  position: absolute;
  top: 0;
  right: -1px;
  bottom: 0;
  width: 1px;
  background: var(--copper-surface);
}

/* 横向：咬合下边 —— 下边框取面板同色，::after 覆盖面板的 1px 上边框。 */
.co-tabs--horizontal .co-tabs__tab--active {
  border-color: var(--copper-border);
  border-bottom-color: var(--copper-surface);
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
}

.co-tabs--horizontal .co-tabs__tab--active::after {
  content: "";
  position: absolute;
  right: 0;
  bottom: -1px;
  left: 0;
  height: 1px;
  background: var(--copper-surface);
}

.co-tabs__label {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.co-tabs__indicator {
  position: absolute;
  top: 0;
  left: 0;
  z-index: 1;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  pointer-events: none;
  transition: transform var(--copper-duration) var(--copper-easing);
}

.co-tabs__indicator--hidden {
  opacity: 0;
}
</style>