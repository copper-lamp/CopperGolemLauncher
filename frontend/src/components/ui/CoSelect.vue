<script setup lang="ts">
// 下拉选择：原生 select 套主题样式。

withDefaults(
  defineProps<{
    modelValue: string;
    options: Array<{ value: string; label: string }>;
    placeholder?: string;
    disabled?: boolean;
  }>(),
  { placeholder: "", disabled: false },
);

const emit = defineEmits<{
  "update:modelValue": [value: string];
}>();
</script>

<template>
  <div :class="['co-select', { 'co-select--disabled': disabled }]">
    <select
      :value="modelValue"
      :disabled="disabled"
      @change="emit('update:modelValue', ($event.target as HTMLSelectElement).value)"
    >
      <option v-if="placeholder && !modelValue" value="" disabled hidden>
        {{ placeholder }}
      </option>
      <option
        v-for="opt in options"
        :key="opt.value"
        :value="opt.value"
      >
        {{ opt.label }}
      </option>
    </select>
  </div>
</template>

<style scoped>
.co-select {
  position: relative;
  display: inline-flex;
  align-items: center;
}

.co-select::after {
  content: "";
  position: absolute;
  right: var(--copper-space-3);
  width: 0;
  height: 0;
  border-left: 4px solid transparent;
  border-right: 4px solid transparent;
  border-top: 5px solid var(--copper-text-secondary);
  pointer-events: none;
}

.co-select select {
  appearance: none;
  -webkit-appearance: none;
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-6) 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  font-family: inherit;
  cursor: pointer;
  transition: border-color var(--copper-duration-fast) var(--copper-easing);
}

.co-select select:hover {
  border-color: var(--copper-accent);
}

.co-select select:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--copper-accent) 40%, transparent);
}

.co-select--disabled {
  opacity: 0.5;
  pointer-events: none;
}
</style>
