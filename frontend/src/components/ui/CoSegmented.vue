<script setup lang="ts">
// 分段选择器：一组互斥选项（实例模式、主题模式、列表密度等）。
//
// 视觉照 planes 的 segmented-control：整条是一枚胶囊轨道，选中项不是"给按钮刷底色"，
// 而是轨道里一枚会滑动的凸起滑块。轨道只比所在表面亮/暗一档（微色阶），滑块用表面色
// 加细阴影浮起，所以整条控件没有描边，项与项之间也没有分隔线。
//
// 滑块位置由 DOM 实测（offsetLeft / offsetWidth），ResizeObserver 跟踪容器尺寸；
// 移动用带轻微回弹的缓动近似 planes 的弹簧，不引入 motion 依赖。首帧定位不开过渡，
// 否则挂载时会看到滑块从 0 宽滑进来。

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

const props = withDefaults(
  defineProps<{
    modelValue: string;
    options: Array<{ value: string; label: string }>;
    disabled?: boolean;
    /** 紧凑档：用在表单行里的次要选择，高度与字号各收一档。 */
    compact?: boolean;
  }>(),
  { disabled: false, compact: false },
);

const emit = defineEmits<{
  "update:modelValue": [value: string];
}>();

const root = ref<HTMLElement | null>(null);
const itemEls: HTMLElement[] = [];
const thumb = ref({ left: 0, width: 0, visible: false });
/** 首帧量完位置之后才允许过渡，避免滑入动画。 */
const animated = ref(false);

const activeIndex = computed(() =>
  props.options.findIndex((option) => option.value === props.modelValue),
);

let observer: ResizeObserver | null = null;
let frame = 0;

function setItem(el: unknown, index: number) {
  if (el instanceof HTMLElement) itemEls[index] = el;
}

/** 量一次选中项的位置。值没变就不写状态，避免无谓的样式重算。 */
function measure() {
  const el = itemEls[activeIndex.value];
  if (!el || !el.offsetWidth) {
    if (thumb.value.visible) thumb.value = { ...thumb.value, visible: false };
    return;
  }
  const next = { left: el.offsetLeft, width: el.offsetWidth, visible: true };
  if (
    next.left === thumb.value.left &&
    next.width === thumb.value.width &&
    next.visible === thumb.value.visible
  ) {
    return;
  }
  thumb.value = next;
}

function measureNextFrame() {
  cancelAnimationFrame(frame);
  frame = requestAnimationFrame(measure);
}

function select(value: string) {
  if (props.disabled || value === props.modelValue) return;
  emit("update:modelValue", value);
}

watch(
  () => props.modelValue,
  () => void nextTick(measure),
);

// 选项数量或文案变化会改变每一项的宽度，滑块要跟着重算。
watch(
  () => props.options,
  () => void nextTick(measureNextFrame),
  { deep: true },
);

watch(
  () => props.compact,
  () => void nextTick(measureNextFrame),
);

onMounted(async () => {
  await nextTick();
  measure();
  requestAnimationFrame(() => {
    // 字体加载完宽度会变一次，量完再开过渡。
    measure();
    animated.value = true;
  });

  // 窗口拉宽 / 侧栏折叠会改按钮宽度；此时直接跳位，不做滑动。
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
    :class="['co-segmented', { 'co-segmented--compact': compact, 'co-segmented--disabled': disabled }]"
  >
    <span
      class="co-segmented__thumb"
      :class="{
        'co-segmented__thumb--visible': thumb.visible,
        'co-segmented__thumb--animated': animated,
      }"
      :style="{ transform: `translateX(${thumb.left}px)`, width: `${thumb.width}px` }"
      aria-hidden="true"
    />
    <button
      v-for="(option, index) in options"
      :key="option.value"
      :ref="(el) => setItem(el, index)"
      type="button"
      :class="['co-segmented__item', { 'co-segmented__item--active': option.value === modelValue }]"
      :aria-pressed="option.value === modelValue"
      :disabled="disabled"
      @click="select(option.value)"
    >
      {{ option.label }}
    </button>
  </div>
</template>

<style scoped>
.co-segmented {
  position: relative;
  display: inline-flex;
  align-items: center;
  max-width: 100%;
  padding: 3px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-segment-track);
  /* 滑块与按钮共处一个层叠上下文：滑块压在按钮下方（z-index 0 / 1），
   * 不会因为父级隔离问题盖住文字。 */
  isolation: isolate;
  user-select: none;
}

/* 滑块本身不接收事件：点按永远落到按钮上。 */
.co-segmented__thumb {
  position: absolute;
  top: 3px;
  bottom: 3px;
  left: 0;
  z-index: 0;
  border-radius: var(--copper-radius-full);
  background: var(--copper-segment-thumb);
  box-shadow: var(--copper-segment-thumb-shadow), var(--copper-segment-thumb-ring);
  opacity: 0;
  pointer-events: none;
  will-change: transform, width;
}

.co-segmented__thumb--visible {
  opacity: 1;
}

.co-segmented__thumb--animated {
  /* 轻微过冲的缓动等价于 planes 的 stiffness 380 / damping 24。 */
  transition:
    transform 380ms cubic-bezier(0.3, 1.25, 0.5, 1),
    width 320ms cubic-bezier(0.3, 1.25, 0.5, 1),
    opacity var(--copper-duration-fast) linear;
}

.co-segmented__item {
  position: relative;
  z-index: 1;
  flex: 1 1 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  min-width: 0;
  height: calc(var(--copper-control-h) - 6px);
  padding: 0 var(--copper-space-4);
  border: none;
  border-radius: var(--copper-radius-full);
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

.co-segmented__item:hover:not(.co-segmented__item--active):not(:disabled) {
  color: var(--copper-text);
}

/* 按下时按钮微微缩小（planes scale .93 的克制版）：反馈来自控件本身，
 * 不靠换底色。 */
.co-segmented__item:active:not(:disabled) {
  transform: scale(0.96);
}

.co-segmented__item--active {
  color: var(--copper-text);
  font-weight: 600;
}

.co-segmented__item:disabled {
  cursor: not-allowed;
}

.co-segmented--disabled {
  opacity: 0.5;
  pointer-events: none;
}

/* 紧凑档：表单行里的次要选择。 */
.co-segmented--compact {
  padding: 2px;
}

.co-segmented--compact .co-segmented__thumb {
  top: 2px;
  bottom: 2px;
}

.co-segmented--compact .co-segmented__item {
  height: 26px;
  padding: 0 var(--copper-space-3);
  font-size: var(--copper-font-size-sm);
}

@media (prefers-reduced-motion: reduce) {
  .co-segmented__thumb--animated,
  .co-segmented__item {
    transition: none;
  }
}
</style>
