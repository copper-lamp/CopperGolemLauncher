<script setup lang="ts">
import { computed, type Component } from "vue";
import { Gamepad2 } from "@lucide/vue";
import CoSelect from "./CoSelect.vue";
const props = withDefaults(defineProps<{ modelValue: string; options: Array<{ value: string; label: string; icon?: string | null }>; icon?: Component; variant?: "pill" | "block"; size?: "lg" | "md"; placeholder?: string; disabled?: boolean }>(), { variant: "pill", size: "lg", placeholder: "", disabled: false });
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const options = computed(() => props.options.map(option => ({ value: option.value, label: option.label, image: option.icon, icon: option.icon ? undefined : props.icon ?? Gamepad2 })));
</script>
<template><CoSelect class="instance-select" :class="[`instance-select--${variant}`, `instance-select--${size}`]" :model-value="modelValue" :options="options" :placeholder="placeholder" :disabled="disabled" @update:model-value="emit('update:modelValue', $event)" /></template>
<style scoped>
.instance-select { width:100%; }
.instance-select--pill { border-radius:var(--copper-radius-full); }
.instance-select--lg { min-height:56px; }
.instance-select--md { min-height:40px; }
</style>
