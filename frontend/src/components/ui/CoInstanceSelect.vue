<script setup lang="ts">
// 实例选择器：胶囊触发条 + 圆角矩形浮层。
//
// 保留专用触发条与实例封面布局，并遵循"最多同屏 5 项、
// 超出滚动"的高度约束。这里的每一项都是「图标 + 名字」，浮层高度按 5 行封顶，
// 实例多时在浮层内部滚动，不会把页面撑长。
//
// 无描边：浮层靠底色阶差 + 阴影与页面分离（本轮统一的视觉规则）。
//
// `variant`：
// - `pill`：首页简洁模式的胶囊面板用，圆角全满；
// - `block`：默认模式启动区用，矩形。
//
// 浮层是绝对定位的兄弟节点，不做 Teleport：触发条所在容器都允许溢出，
// 而 Teleport 会把焦点顺序打乱（键盘用户要先跳出触发条才能进浮层）。

import { computed, nextTick, onBeforeUnmount, ref, watch, type Component } from "vue";
import { ChevronDown, Gamepad2 } from "@lucide/vue";

const props = withDefaults(
  defineProps<{
    modelValue: string;
    /** 选项：`icon` 为实例封面的 data URL，缺失时用占位图标。 */
    options: Array<{ value: string; label: string; icon?: string | null }>;
    /** 占位图标（实例用 Gamepad2，客户端用 PlugZap）。 */
    icon?: Component;
    variant?: "pill" | "block";
    size?: "lg" | "md";
    placeholder?: string;
    disabled?: boolean;
  }>(),
  { variant: "pill", size: "lg", placeholder: "", disabled: false },
);

const emit = defineEmits<{
  "update:modelValue": [value: string];
}>();

const open = ref(false);
const root = ref<HTMLElement | null>(null);

const current = computed(
  () => props.options.find((option) => option.value === props.modelValue) ?? null,
);

const selectIcon = computed(() => props.icon ?? Gamepad2);

function close() {
  open.value = false;
}

function toggle() {
  if (props.disabled) return;
  open.value = !open.value;
}

function pick(value: string) {
  if (props.disabled) return;
  close();
  root.value?.querySelector<HTMLButtonElement>(".instance-select__trigger")?.focus();
  if (value === props.modelValue) return;
  emit("update:modelValue", value);
}

function onDocumentPointerDown(event: PointerEvent) {
  if (!open.value) return;
  const target = event.target as Node | null;
  if (root.value && target && !root.value.contains(target)) close();
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape" && open.value) {
    close();
    root.value?.querySelector<HTMLButtonElement>(".instance-select__trigger")?.focus();
  }
}

watch(() => props.disabled, disabled => { if (disabled) close(); });

watch(open, (value) => {
  if (value) {
    document.addEventListener("pointerdown", onDocumentPointerDown);
    document.addEventListener("keydown", onKeydown);
  } else {
    document.removeEventListener("pointerdown", onDocumentPointerDown);
    document.removeEventListener("keydown", onKeydown);
  }
});

onBeforeUnmount(() => {
  document.removeEventListener("pointerdown", onDocumentPointerDown);
  document.removeEventListener("keydown", onKeydown);
});

async function onControlKeydown(event: KeyboardEvent) {
  if (props.disabled || !["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  open.value = true;
  await nextTick();
  const buttons = Array.from(root.value?.querySelectorAll<HTMLButtonElement>(".instance-select__option:not(:disabled)") ?? []);
  if (!buttons.length) return;
  const currentIndex = buttons.indexOf(document.activeElement as HTMLButtonElement);
  const index = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
    : currentIndex < 0 ? (event.key === "ArrowUp" ? buttons.length - 1 : 0)
    : (currentIndex + (event.key === "ArrowUp" ? -1 : 1) + buttons.length) % buttons.length;
  buttons[index]?.focus();
}
</script>

<template>
  <div
    ref="root"
    @keydown="onControlKeydown"
    @focusout="event => { if (!root?.contains(event.relatedTarget as Node | null)) open = false; }"
    :class="[
      'instance-select',
      `instance-select--${variant}`,
      `instance-select--${size}`,
      { 'instance-select--open': open, 'instance-select--disabled': disabled },
    ]"
  >
    <button
      type="button"
      class="instance-select__trigger"
      :aria-expanded="open"
      aria-haspopup="listbox"
      :disabled="disabled"
      @click="toggle"
    >
      <span class="instance-select__icon">
        <img v-if="current?.icon" :src="current.icon" alt="" />
        <component :is="selectIcon" v-else :size="20" :stroke-width="1.6" />
      </span>
      <span class="instance-select__label" :title="current?.label ?? placeholder">
        {{ current?.label ?? placeholder }}
      </span>
      <ChevronDown :size="16" :stroke-width="2" class="instance-select__chevron" />
    </button>

    <Transition name="instance-select-pop">
      <div v-if="open" class="instance-select__menu" role="listbox">
        <button
          v-for="option in options"
          :key="option.value"
          type="button"
          :class="[
            'instance-select__option',
            { 'instance-select__option--active': option.value === modelValue },
          ]"
          role="option"
          :aria-selected="option.value === modelValue"
          @click="pick(option.value)"
        >
          <span class="instance-select__icon">
            <img v-if="option.icon" :src="option.icon" alt="" />
            <component :is="selectIcon" v-else :size="20" :stroke-width="1.6" />
          </span>
          <span class="instance-select__option-label" :title="option.label">{{ option.label }}</span>
        </button>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.instance-select {
  position: relative;
  min-width: 0;
}

/* 触发条：无描边，靠底色阶差与页面分离。 */
.instance-select__trigger {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  width: 100%;
  min-width: 0;
  border: none;
  background: transparent;
  color: var(--copper-text);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.instance-select--pill .instance-select__trigger {
  height: 56px;
  padding: 0 var(--copper-space-4) 0 6px;
  border-radius: var(--copper-radius-full);
}

.instance-select--block .instance-select__trigger {
  height: 56px;
  padding: 0 var(--copper-space-3);
  border-radius: var(--copper-radius-md);
}

.instance-select--md .instance-select__trigger {
  height: 40px;
}

.instance-select__trigger:hover:not(:disabled) {
  background: var(--copper-hover);
}

.instance-select__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  overflow: hidden;
}

.instance-select--md .instance-select__icon {
  width: 28px;
  height: 28px;
}

.instance-select__icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.instance-select__label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  font-size: 16px;
  font-weight: 500;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.instance-select--md .instance-select__label {
  font-size: var(--copper-font-size-md);
}

.instance-select__chevron {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
  transition: transform var(--copper-duration) var(--copper-easing);
}

.instance-select--open .instance-select__chevron {
  transform: rotate(180deg);
}

/* 浮层：圆角矩形，最多同屏 5 项（5 × 52 + 内边距），超出内部滚动。 */
.instance-select__menu {
  position: absolute;
  left: 0;
  right: 0;
  top: calc(100% + 8px);
  z-index: 40;
  max-height: 280px;
  overflow-y: auto;
  overscroll-behavior: contain;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px;
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow);
}

.instance-select__option {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  flex-shrink: 0;
  height: 52px;
  padding: 0 var(--copper-space-2);
  border: none;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text);
  font-family: inherit;
  font-size: 15px;
  text-align: left;
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.instance-select__option:hover {
  background: var(--copper-hover);
}

.instance-select__option--active {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
}

.instance-select__option-label {
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.instance-select--disabled {
  opacity: 0.55;
  pointer-events: none;
}

.instance-select-pop-enter-active,
.instance-select-pop-leave-active {
  transition:
    opacity var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.instance-select-pop-enter-from,
.instance-select-pop-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}
.instance-select__option:focus-visible, .instance-select__trigger:focus-visible { outline: 2px solid var(--copper-accent); outline-offset: -2px; }
</style>