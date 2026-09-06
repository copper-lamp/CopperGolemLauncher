<script setup lang="ts">
// 开关：受控组件，`modelValue` 双向绑定。

const props = defineProps<{
  modelValue: boolean;
  disabled?: boolean;
  label?: string;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: boolean];
}>();

function toggle() {
  if (props.disabled) return;
  emit("update:modelValue", !props.modelValue);
}
</script>

<template>
  <button
    :class="['co-switch', { 'co-switch--on': modelValue, 'co-switch--disabled': disabled }]"
    role="switch"
    :aria-checked="modelValue"
    :aria-label="label"
    @click="toggle"
  >
    <span class="co-switch__thumb" />
  </button>
</template>

<style scoped>
.co-switch {
  position: relative;
  width: 38px;
  height: 22px;
  flex-shrink: 0;
  border: none;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-3);
  cursor: pointer;
  transition: background-color var(--copper-duration) var(--copper-easing);
}

.co-switch--on {
  background: var(--copper-accent);
}

.co-switch--disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.co-switch__thumb {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 18px;
  height: 18px;
  border-radius: var(--copper-radius-full);
  background: #ffffff;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25);
  transition: transform var(--copper-duration) var(--copper-easing);
}

.co-switch--on .co-switch__thumb {
  transform: translateX(16px);
}
</style>
