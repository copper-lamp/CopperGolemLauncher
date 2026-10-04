<script setup lang="ts">
// 下拉选择：折叠态是「标签 + 当前值」的一整行，展开后**组件自身变长**，选项就排
// 在这个组件内部（不是向下弹出的浮层）。
//
// 与 `CoSelect`（原生 select 套主题样式）的分工：原生 select 的弹层由系统绘制，
// 无法承载图标与逐项说明，也无法保证与主题一致的悬停反馈。需要「图标 + 名字」这类
// 富内容选项时用本组件；选项少、纯文本、要系统级无障碍行为时仍用 `CoSelect`。
//
// 交互约定：
// - 折叠态占满容器宽度，左侧标签、右侧当前值，无值时显示占位文案（如「未选择」）；
// - 展开时行高由控制件高度过渡到列表自然高度，选项为**无描边**按钮，悬停时才出现阴影；
// - **无描边**：整个组件靠底色差（`--copper-surface` 与页面背景 `--copper-bg`）区分，
//   描边在这个页面上只会把「加载器 / 客户端」两块控件框成一堆表单边框；
// - 选择某项后自动收起；点击组件外部或按 Esc 收起；禁用时不可展开。
//
// 为什么不用浮层：浮层会被父级 `overflow: hidden` 裁掉、在窄屏上横向溢出，还会挡住
// 下方的内容；内联展开让组件始终留在文档流里，滚动与布局行为都可预期。

import type { Component } from "vue";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { ChevronDown } from "@lucide/vue";

/** 一个选项：图标可选，名字必填。 */
export interface DropdownOption {
  value: string;
  label: string;
  /** 选项图标（lucide 组件）。 */
  icon?: Component;
  /** 右侧补充说明（如版本要求）；为空不显示。 */
  hint?: string;
}

const props = withDefaults(
  defineProps<{
    modelValue: string;
    /** 折叠态左侧的标签（如「加载器」）。 */
    label: string;
    options: DropdownOption[];
    /** 未选中时的占位文案。 */
    placeholder?: string;
    /** 选项为空时面板里的提示。 */
    emptyText?: string;
    disabled?: boolean;
  }>(),
  { placeholder: "", emptyText: "", disabled: false },
);

const emit = defineEmits<{
  "update:modelValue": [value: string];
}>();

const open = ref(false);
const root = ref<HTMLElement | null>(null);
const list = ref<HTMLElement | null>(null);
/** 列表可滚动时，上/下边缘是否还有被截断的内容（决定要不要加渐变遮罩）。 */
const fadeTop = ref(false);
const fadeBottom = ref(false);

const selected = computed(
  () => props.options.find((option) => option.value === props.modelValue) ?? null,
);
/** 折叠态显示的名字：选中项的 label，未选中时用占位文案。 */
const currentLabel = computed(() => selected.value?.label ?? props.placeholder);
const isEmpty = computed(() => props.options.length === 0);

function toggle() {
  if (props.disabled) return;
  open.value = !open.value;
}

/**
 * 依据滚动位置刷新上下渐变遮罩。
 *
 * 内容比可视高度长时（LeviLamina 版本库动辄几十项），列表上下边缘会「硬生生」
 * 切掉半个选项——没有渐变的话用户根本看不出下面还有东西。加渐变的判定必须来自
 * 真实滚动位置：无溢出时不加（否则第一项和最后一项会平白变淡），只有那一侧确实
 * 还有内容被截断时才加。
 */
function updateFade() {
  const el = list.value;
  if (!el) {
    fadeTop.value = false;
    fadeBottom.value = false;
    return;
  }
  const max = el.scrollHeight - el.clientHeight;
  fadeTop.value = max > 1 && el.scrollTop > 1;
  fadeBottom.value = max > 1 && el.scrollTop < max - 1;
}

/** 展开后等布局稳定再判定渐变（收起态内容高度为 0，判定不出结果）。 */
watch(open, async (isOpen) => {
  if (!isOpen) return;
  await nextTick();
  const el = list.value;
  if (el) el.scrollTop = 0;
  updateFade();
});

function choose(option: DropdownOption) {
  emit("update:modelValue", option.value);
  open.value = false;
}

/** 点击组件之外收起：面板会持续占据布局空间，不收起会把下面的内容一直挤下去。 */
function onDocumentPointerDown(event: MouseEvent) {
  if (!open.value) return;
  if (root.value && !root.value.contains(event.target as Node)) open.value = false;
}

function onDocumentKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") open.value = false;
}

onMounted(() => {
  document.addEventListener("mousedown", onDocumentPointerDown);
  document.addEventListener("keydown", onDocumentKeydown);
});

onBeforeUnmount(() => {
  document.removeEventListener("mousedown", onDocumentPointerDown);
  document.removeEventListener("keydown", onDocumentKeydown);
});

// 禁用时收起面板，避免留下一个挡住内容的展开区。
// 选项为空**不**收起：空态要说「暂无可用加载器」，直接关掉等于什么都不说。
watch(
  () => props.disabled,
  (disabled) => {
    if (disabled) open.value = false;
  },
);
</script>

<template>
  <div
    ref="root"
    class="co-dropdown"
    :class="{ 'co-dropdown--open': open, 'co-dropdown--disabled': disabled }"
  >
    <button
      type="button"
      class="co-dropdown__trigger"
      :disabled="disabled"
      :aria-expanded="open"
      @click="toggle"
    >
      <span class="co-dropdown__label">{{ label }}</span>
      <span class="co-dropdown__value">{{ currentLabel }}</span>
      <ChevronDown :size="16" class="co-dropdown__chevron" />
    </button>

    <div class="co-dropdown__panel" role="listbox">
      <!-- 两层：__panel-inner 只负责被折叠（必须零内边距，见样式注释），
           __list 才是真正有内容、会滚动、会渐变的那一层。 -->
      <div ref="panelInner" class="co-dropdown__panel-inner">
        <div
          ref="list"
          class="co-dropdown__list"
          :class="{
            'co-dropdown__list--fade-top': fadeTop,
            'co-dropdown__list--fade-bottom': fadeBottom,
          }"
          @scroll="updateFade"
        >
          <p v-if="isEmpty" class="co-dropdown__empty">{{ emptyText }}</p>
          <button
            v-for="option in options"
            :key="option.value"
            type="button"
            role="option"
            :aria-selected="option.value === modelValue"
            class="co-dropdown__option"
            :class="{ 'co-dropdown__option--selected': option.value === modelValue }"
            @click="choose(option)"
          >
            <component
              :is="option.icon"
              v-if="option.icon"
              :size="18"
              class="co-dropdown__option-icon"
            />
            <span class="co-dropdown__option-label">{{ option.label }}</span>
            <span v-if="option.hint" class="co-dropdown__option-hint">{{ option.hint }}</span>
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.co-dropdown {
  position: relative;
  width: 100%;
  /* 展开态圆角内收：否则折叠行与列表拼接处会出现一个方肩。 */
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  transition: box-shadow var(--copper-duration) var(--copper-easing);
}

.co-dropdown--open {
  box-shadow: var(--copper-shadow);
}

/* 触发行比标准控件高一档：这个组件承载「图标 + 名字」两类信息，太窄会挤成两行。 */
.co-dropdown__trigger {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  width: 100%;
  height: var(--copper-control-h-lg);
  padding: 0 var(--copper-space-4);
  /* 无描边：与页面背景的底色差负责分组。 */
  border: 1px solid transparent;
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-lg);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.co-dropdown__trigger:hover:not(:disabled) {
  background: var(--copper-surface-2);
}

.co-dropdown__trigger:focus-visible {
  outline: none;
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--copper-accent) 22%, transparent);
}

.co-dropdown--open .co-dropdown__trigger {
  /* 展开时下方就是列表，触发行不再自带圆角。 */
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
}

.co-dropdown__label {
  color: var(--copper-text-secondary);
  flex-shrink: 0;
}

.co-dropdown__value {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.co-dropdown__chevron {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
  transition: transform var(--copper-duration) var(--copper-easing);
}

.co-dropdown--open .co-dropdown__chevron {
  transform: rotate(180deg);
}

.co-dropdown--disabled .co-dropdown__trigger {
  cursor: default;
}

/* 组件自身变长：0fr → 1fr，无需知道选项有多少个。 */
.co-dropdown__panel {
  display: grid;
  grid-template-rows: 0fr;
  transition: grid-template-rows var(--copper-duration) var(--copper-easing);
}

.co-dropdown--open .co-dropdown__panel {
  grid-template-rows: 1fr;
}

.co-dropdown--open .co-dropdown__panel-inner {
  /* 展开时立即可见（延迟为 0），收起时才等动画结束再隐藏。 */
  visibility: visible;
  transition-delay: 0s;
}

/* 真正有内容的一层：内边距、滚动上限、上下渐变都在这里。
 *
 * 关键：**内边距不能写在被折叠的那一层**。`grid-template-rows: 0fr` 的自动
 * 最小尺寸是 min-content，而 min-content 里包含内边距——内边距写在折叠层上时，
 * 折叠后触发行下方仍会露出「一条内边距高」的浅色残影（用户报的「脚」）。
 * 折叠层保持零内边距、零内容，min-content 才是真正的 0。 */
.co-dropdown__panel-inner {
  min-height: 0;
  overflow: hidden;
  visibility: hidden;
  /* 收起后 visibility:hidden 而不是 display:none —— 前者能在收起动画播完之后
   * 把选项移出无障碍树与 Tab 序（display:none 会直接砍掉过渡）。 */
  transition: visibility 0s linear var(--copper-duration);
}

.co-dropdown__list {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
  /* 列表独立滚动：加载器版本库可能有几十项，不限高就会把页面顶长。
   * 上限同时受视口约束，短窗口下也不会反过来顶出页面。 */
  max-height: min(320px, 45vh);
  overflow-y: auto;
  padding: var(--copper-space-1) var(--copper-space-2) var(--copper-space-2);
  /* **无描边**：这个控件在页面里靠底色差分组，任何一圈线都会读成表单边框。 */
  border: none;
}

/* 上下渐变：那一侧确实还有内容被截断时，用「淡出到透明」提示还能滚。
 * mask 同时作用于所有子元素，视觉上就是内容渐隐，而不是盖一层灰条。 */
.co-dropdown__list--fade-top {
  mask-image: linear-gradient(to bottom, transparent, #000 18px);
}

.co-dropdown__list--fade-bottom {
  mask-image: linear-gradient(to top, transparent, #000 18px);
}

/* 两侧同时被截断：上下各一段渐隐，中间保持不透明。 */
.co-dropdown__list--fade-top.co-dropdown__list--fade-bottom {
  mask-image: linear-gradient(
    to bottom,
    transparent,
    #000 18px,
    #000 calc(100% - 18px),
    transparent
  );
}

.co-dropdown__empty {
  margin: 0;
  padding: var(--copper-space-2) var(--copper-space-3);
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-disabled);
}

/* 选项：无描边，悬停时才出现阴影。 */
.co-dropdown__option {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  width: 100%;
  padding: var(--copper-space-2) var(--copper-space-3);
  border: 1px solid transparent;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.co-dropdown__option:hover {
  background: var(--copper-hover);
  box-shadow: var(--copper-shadow);
}

.co-dropdown__option:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--copper-accent) 45%, transparent);
  outline-offset: -1px;
}

.co-dropdown__option--selected {
  color: var(--copper-accent);
}

.co-dropdown__option-icon {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
}

.co-dropdown__option--selected .co-dropdown__option-icon {
  color: var(--copper-accent);
}

.co-dropdown__option-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.co-dropdown__option-hint {
  flex-shrink: 0;
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}
</style>
