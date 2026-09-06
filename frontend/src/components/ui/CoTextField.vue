<script setup lang="ts">
// 文本输入框。

withDefaults(
  defineProps<{
    modelValue: string;
    placeholder?: string;
    disabled?: boolean;
    type?: "text" | "password";
  }>(),
  { placeholder: "", disabled: false, type: "text" },
);

const emit = defineEmits<{
  "update:modelValue": [value: string];
  enter: [];
}>();
</script>

<template>
  <input
    :type="type"
    :value="modelValue"
    :placeholder="placeholder"
    :disabled="disabled"
    class="co-text-field"
    @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)"
    @keydown.enter="emit('enter')"
  />
</template>

<style scoped>
.co-text-field {
  width: 100%;
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  font-family: inherit;
  transition:
    border-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.co-text-field::placeholder {
  color: var(--copper-text-disabled);
}

.co-text-field:hover {
  border-color: color-mix(in srgb, var(--copper-accent) 50%, var(--copper-border));
}

.co-text-field:focus-visible {
  outline: none;
  border-color: var(--copper-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--copper-accent) 22%, transparent);
}

.co-text-field:disabled {
  opacity: 0.5;
}
</style>
