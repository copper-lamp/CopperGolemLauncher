<script setup lang="ts">
withDefaults(defineProps<{ modelValue: string; placeholder?: string; disabled?: boolean; type?: "text" | "password" }>(), { placeholder: "", disabled: false, type: "text" });
const emit = defineEmits<{ "update:modelValue": [value: string]; enter: [] }>();
</script>
<template><input :type="type" :value="modelValue" :placeholder="placeholder" :disabled="disabled" class="co-text-field" @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)" @keydown.enter="emit('enter')" /></template>
<style scoped>
.co-text-field { width:100%;height:var(--copper-control-h);padding:0 var(--copper-space-3);border:0;border-radius:var(--copper-radius-control);background:var(--copper-surface-2);color:var(--copper-text);font-size:var(--copper-font-size-md);font-family:inherit;box-shadow:var(--copper-shadow-subtle);transition:background var(--copper-duration-fast),box-shadow var(--copper-duration-fast); }
.co-text-field::placeholder { color:var(--copper-text-disabled); }
.co-text-field:hover:not(:disabled) { background:var(--copper-surface-3); }
.co-text-field:focus-visible { outline:2px solid var(--copper-accent);outline-offset:2px; }
.co-text-field:disabled { opacity:.5; }
</style>
