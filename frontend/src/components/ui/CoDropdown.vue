<script setup lang="ts">
import type { Component } from "vue";
import CoSelect from "./CoSelect.vue";
export interface DropdownOption { value: string; label: string; icon?: Component; hint?: string; disabled?: boolean }
withDefaults(defineProps<{ modelValue: string; label: string; options: DropdownOption[]; placeholder?: string; emptyText?: string; disabled?: boolean }>(), { placeholder: "", emptyText: "", disabled: false });
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
</script>
<template><CoSelect class="co-dropdown" :model-value="modelValue" :label="label" :options="options" :placeholder="placeholder" :empty-text="emptyText" :disabled="disabled" @update:model-value="emit('update:modelValue', $event)" /></template>
<style scoped>.co-dropdown { width:100%; min-height:var(--copper-control-h-lg); }</style>
