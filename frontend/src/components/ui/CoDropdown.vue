<script setup lang="ts">
// 下拉选择：折叠态是「标签 + 当前值」的一整行，展开后向下弹出选项面板。
//
// 与 `CoSelect`（原生 select 套主题样式）的分工：原生 select 的弹层由系统绘制，
// 无法承载图标与逐项说明，也无法保证与主题一致的悬停反馈。需要「图标 + 名字」这类
// 富内容选项时用本组件；选项少、纯文本、要系统级无障碍行为时仍用 `CoSelect`。
//
// 交互约定：
// - 折叠态占满容器宽度，左侧标签、右侧当前值，无值时显示占位文案（如「未选择」）；
// - 展开态在下方弹出，选项为**无描边**按钮，悬停时才出现阴影；
// - 点击组件外部或按 Esc 收起；禁用时不可展开。

import type { Component } from "vue";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
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

function choose(option: DropdownOption) {
  emit("update:modelValue", option.value);
  open.value = false;
}

/** 点击组件之外收起：面板是绝对定位的浮层，不收起会一直挡住下面的内容。 */
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

// 禁用时收起面板，避免留下一个悬空浮层。
// 选项为空**不**收起：空态面板要说「暂无可用加载器」，直接关掉等于什么都不说。
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
      <ChevronDown :size="15" class="co-dropdown__chevron" />
    </button>

    <Transition name="co-dropdown-panel">
      <div v-if="open" class="co-dropdown__panel" role="listbox">
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
          <component :is="option.icon" v-if="option.icon" :size="16" class="co-dropdown__option-icon" />
          <span class="co-dropdown__option-label">{{ option.label }}</span>
          <span v-if="option.hint" class="co-dropdown__option-hint">{{ option.hint }}</span>
        </button>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.co-dropdown {
  position: relative;
  width: 100%;
}

.co-dropdown__trigger {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  width: 100%;
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transition:
    border-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.co-dropdown__trigger:hover:not(:disabled) {
  border-color: color-mix(in srgb, var(--copper-accent) 50%, var(--copper-border));
}

.co-dropdown__trigger:focus-visible {
  outline: none;
  border-color: var(--copper-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--copper-accent) 22%, transparent);
}

.co-dropdown--open .co-dropdown__trigger {
  border-color: var(--copper-accent);
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

.co-dropdown__panel {
  position: absolute;
  z-index: 40;
  top: calc(100% + var(--copper-space-1));
  left: 0;
  right: 0;
  max-height: 260px;
  overflow-y: auto;
  padding: var(--copper-space-1);
  display: flex;
  flex-direction: column;
  gap: 2px;
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  box-shadow: var(--copper-shadow);
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

.co-dropdown-panel-enter-active,
.co-dropdown-panel-leave-active {
  transition:
    opacity var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.co-dropdown-panel-enter-from,
.co-dropdown-panel-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}
</style>
