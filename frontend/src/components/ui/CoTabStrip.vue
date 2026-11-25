<script setup lang="ts">
// 下划线标签条：一组互斥的页内视图切换（内容详情的「版本 / 简介」）。
//
// 视觉只靠文字与一条渐变的选中指示线：没有容器底色、没有整行轨道线、没有边框，
// 与全站「微色阶 + 阴影」的语言一致。指示线位置由 DOM 实测（offsetLeft / offsetWidth），
// ResizeObserver 跟踪容器与字体宽度；首帧量完再开过渡，否则挂载时会看到从 0 宽滑入。
//
// 键盘按 WAI-ARIA tabs 的自动激活模式：焦点移动即切换，←/→ 循环，Home/End 跳首尾。

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch, type Component } from "vue";

interface TabItem {
  value: string;
  label: string;
  icon?: Component;
  /** 对应面板的 DOM id（用于 aria-controls）；没有面板时省略。 */
  panelId?: string;
}

const props = withDefaults(
  defineProps<{
    modelValue: string;
    items: TabItem[];
    disabled?: boolean;
    /** 标签多于一行时允许横向滚动（窄窗口）。 */
    scrollable?: boolean;
  }>(),
  { disabled: false, scrollable: false },
);

const emit = defineEmits<{
  "update:modelValue": [value: string];
}>();

let tabStripSeq = 0;
const uid = ++tabStripSeq;

const root = ref<HTMLElement | null>(null);
const itemEls: HTMLElement[] = [];
const indicator = ref({ left: 0, width: 0, visible: false });
/** 首帧量完位置之后才允许过渡。 */
const animated = ref(false);

const activeIndex = computed(() =>
  props.items.findIndex((item) => item.value === props.modelValue),
);

const indicatorStyle = computed(() => ({
  transform: "translateX(" + indicator.value.left + "px)",
  width: indicator.value.width + "px",
}));

let observer: ResizeObserver | null = null;
let frame = 0;

function tabId(value: string) {
  return "co-tab-" + uid + "-" + value;
}

function setItem(el: unknown, index: number) {
  if (el instanceof HTMLElement) itemEls[index] = el;
}

/** 量一次选中项的位置；值没变就不写状态，避免无谓的样式重算。 */
function measure() {
  const el = itemEls[activeIndex.value];
  if (!el || !el.offsetWidth) {
    if (indicator.value.visible) indicator.value = { ...indicator.value, visible: false };
    return;
  }
  const next = { left: el.offsetLeft, width: el.offsetWidth, visible: true };
  if (
    next.left === indicator.value.left &&
    next.width === indicator.value.width &&
    next.visible === indicator.value.visible
  ) {
    return;
  }
  indicator.value = next;
}

function measureNextFrame() {
  cancelAnimationFrame(frame);
  frame = requestAnimationFrame(measure);
}

function select(value: string) {
  if (props.disabled || value === props.modelValue) return;
  emit("update:modelValue", value);
}

function focusTab(index: number) {
  const el = itemEls[index];
  if (!el) return;
  el.focus();
  select(props.items[index].value);
}

function onKeydown(event: KeyboardEvent) {
  if (props.disabled || !props.items.length) return;
  const last = props.items.length - 1;
  const current = activeIndex.value < 0 ? 0 : activeIndex.value;
  let next = -1;
  switch (event.key) {
    case "ArrowRight":
    case "ArrowDown":
      next = current >= last ? 0 : current + 1;
      break;
    case "ArrowLeft":
    case "ArrowUp":
      next = current <= 0 ? last : current - 1;
      break;
    case "Home":
      next = 0;
      break;
    case "End":
      next = last;
      break;
    default:
      return;
  }
  event.preventDefault();
  focusTab(next);
}

watch(
  () => props.modelValue,
  () => void nextTick(measure),
);

// 选项数量或文案变化会改变每一项的宽度，指示线要跟着重算。
watch(
  () => props.items,
  () => void nextTick(measureNextFrame),
  { deep: true },
);

onMounted(async () => {
  await nextTick();
  measure();
  requestAnimationFrame(() => {
    // 字体加载完宽度会变一次，量完再开过渡。
    measure();
    animated.value = true;
  });
  // 窗口拉宽 / 侧栏折叠会改标签宽度；此时直接跳位，不做滑动。
  if (typeof ResizeObserver !== "undefined" && root.value) {
    observer = new ResizeObserver(() => measureNextFrame());
    observer.observe(root.value);
  }
  void document.fonts?.ready.then(() => measureNextFrame());
});

onBeforeUnmount(() => {
  cancelAnimationFrame(frame);
  observer?.disconnect();
  observer = null;
});
</script>

<template>
  <div
    ref="root"
    :class="[
      'co-tabstrip',
      {
        'co-tabstrip--scrollable': scrollable,
        'co-tabstrip--disabled': disabled,
      },
    ]"
    role="tablist"
    aria-orientation="horizontal"
    @keydown="onKeydown"
  >
    <span
      class="co-tabstrip__indicator"
      :class="{
        'co-tabstrip__indicator--visible': indicator.visible,
        'co-tabstrip__indicator--animated': animated,
      }"
      :style="indicatorStyle"
      aria-hidden="true"
    />
    <button
      v-for="(item, index) in items"
      :id="tabId(item.value)"
      :key="item.value"
      :ref="(el) => setItem(el, index)"
      type="button"
      role="tab"
      :class="['co-tabstrip__tab', { 'is-active': item.value === modelValue }]"
      :aria-selected="item.value === modelValue"
      :aria-controls="item.panelId"
      :tabindex="item.value === modelValue ? 0 : -1"
      :disabled="disabled"
      @click="select(item.value)"
    >
      <component :is="item.icon" v-if="item.icon" :size="14" class="co-tabstrip__icon" />
      <span class="co-tabstrip__label">{{ item.label }}</span>
    </button>
  </div>
</template>

<style scoped>
.co-tabstrip {
  position: relative;
  display: flex;
  align-items: stretch;
  gap: var(--copper-space-5);
  min-width: 0;
  /* 指示线与标签共处一个层叠上下文，不会互相压字。 */
  isolation: isolate;
  user-select: none;
}

.co-tabstrip--scrollable {
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
}

.co-tabstrip--scrollable::-webkit-scrollbar {
  display: none;
}

/* 选中指示线：一条从 accent 渐隐到透明的细线，代替整行轨道。 */
.co-tabstrip__indicator {
  position: absolute;
  left: 0;
  bottom: 0;
  z-index: 0;
  height: 2px;
  border-radius: 2px;
  background: linear-gradient(
    90deg,
    var(--copper-accent),
    color-mix(in srgb, var(--copper-accent) 35%, transparent)
  );
  box-shadow: 0 0 10px color-mix(in srgb, var(--copper-accent) 45%, transparent);
  opacity: 0;
  pointer-events: none;
  will-change: transform, width;
}

.co-tabstrip__indicator--visible {
  opacity: 1;
}

.co-tabstrip__indicator--animated {
  /* 与分段控件同一条缓动：轻微过冲后落定。 */
  transition:
    transform 380ms cubic-bezier(0.3, 1.25, 0.5, 1),
    width 320ms cubic-bezier(0.3, 1.25, 0.5, 1),
    opacity var(--copper-duration-fast) linear;
}

.co-tabstrip__tab {
  position: relative;
  z-index: 1;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex: 0 0 auto;
  height: calc(var(--copper-control-h) + 6px);
  padding: 0 1px;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
  cursor: pointer;
  transition:
    color var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

/* hover 提示线：只在未选中项上淡入，给「可点」的暗示。 */
.co-tabstrip__tab::after {
  content: "";
  position: absolute;
  left: 1px;
  right: 1px;
  bottom: 0;
  height: 2px;
  border-radius: 2px;
  background: var(--copper-surface-3);
  opacity: 0;
  transition: opacity var(--copper-duration-fast) var(--copper-easing);
}

.co-tabstrip__tab:hover:not(.is-active):not(:disabled) {
  color: var(--copper-text);
}

.co-tabstrip__tab:hover:not(.is-active):not(:disabled)::after {
  opacity: 0.9;
}

.co-tabstrip__tab:active:not(:disabled) {
  transform: translateY(1px);
}

.co-tabstrip__tab.is-active {
  color: var(--copper-text);
  font-weight: 600;
}

.co-tabstrip__tab:focus-visible {
  outline: 2px solid var(--copper-accent);
  outline-offset: 3px;
  border-radius: var(--copper-radius-sm);
}

.co-tabstrip__icon {
  flex: 0 0 auto;
  opacity: 0.8;
}

.co-tabstrip__tab.is-active .co-tabstrip__icon {
  color: var(--copper-accent);
  opacity: 1;
}

.co-tabstrip__label {
  overflow-wrap: anywhere;
}

.co-tabstrip--disabled {
  opacity: 0.5;
  pointer-events: none;
}

@media (prefers-reduced-motion: reduce) {
  .co-tabstrip__indicator--animated,
  .co-tabstrip__tab,
  .co-tabstrip__tab::after {
    transition: none;
  }
}
</style>
