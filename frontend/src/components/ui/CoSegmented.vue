<script setup lang="ts">
// 分段选择器：互斥选项（外观模式、列表密度等）。

const props = withDefaults(
  defineProps<{
    modelValue: string;
    options: Array<{ value: string; label: string }>;
    disabled?: boolean;
  }>(),
  { disabled: false },
);

const emit = defineEmits<{
  "update:modelValue": [value: string];
}>();

function select(value: string) {
  if (value === props.modelValue) return;
  emit("update:modelValue", value);
}
</script>

<template>
  <div :class="['co-segmented', { 'co-segmented--disabled': disabled }]">
    <button
      v-for="opt in options"
      :key="opt.value"
      :class="['co-segmented__item', { 'co-segmented__item--active': modelValue === opt.value }]"
      @click="select(opt.value)"
    >
      {{ opt.label }}
    </button>
  </div>
</template>

<style scoped>
.co-segmented {
  display: inline-flex;
  padding: 3px;
  gap: 2px;
  background: var(--copper-surface-2);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
}

.co-segmented__item {
  height: calc(var(--copper-control-h) - 8px);
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: calc(var(--copper-radius-md) - 2px);
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.co-segmented__item:hover:not(.co-segmented__item--active) {
  color: var(--copper-text);
}

.co-segmented__item--active {
  background: var(--copper-surface);
  color: var(--copper-text);
  font-weight: 500;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
}

.co-segmented--disabled {
  opacity: 0.5;
  pointer-events: none;
}
</style>
