<script setup lang="ts">
withDefaults(defineProps<{ variant?: "primary" | "secondary" | "ghost" | "danger"; size?: "md" | "sm"; disabled?: boolean; type?: "button" | "submit" }>(), { variant: "secondary", size: "md", disabled: false, type: "button" });
const emit = defineEmits<{ click: [event: MouseEvent] }>();
</script>
<template><button :type="type" :disabled="disabled" :class="['co-btn', `co-btn--${variant}`, `co-btn--${size}`]" @click="emit('click', $event)"><slot /></button></template>
<style scoped>
.co-btn { display:inline-flex;align-items:center;justify-content:center;gap:var(--copper-space-2);border:0;border-radius:var(--copper-radius-control);font-weight:500;cursor:pointer;transition:background-color var(--copper-duration-fast),color var(--copper-duration-fast),transform var(--copper-motion-component-press) var(--copper-easing-jelly);white-space:nowrap;box-shadow:var(--copper-shadow-raised); }
.co-btn--md { height:var(--copper-control-h);padding:0 var(--copper-space-4);font-size:var(--copper-font-size-md); }
.co-btn--sm { height:var(--copper-control-h-sm);padding:0 var(--copper-space-3);font-size:var(--copper-font-size-sm); }
.co-btn:active:not(:disabled) { transform:scale(.97); }
.co-btn:disabled { opacity:.5;cursor:not-allowed; }
.co-btn--primary { background:var(--copper-accent);color:var(--copper-accent-foreground); }
.co-btn--primary:hover:not(:disabled) { filter:brightness(1.06); }
.co-btn--secondary { background:var(--copper-surface-2);color:var(--copper-text); }
.co-btn--secondary:hover:not(:disabled) { background:var(--copper-surface-3); }
.co-btn--ghost { background:transparent;color:var(--copper-text-secondary);box-shadow:none; }
.co-btn--ghost:hover:not(:disabled) { background:var(--copper-hover);color:var(--copper-text); }
.co-btn--danger { background:color-mix(in srgb,var(--copper-danger) 10%,var(--copper-surface));color:var(--copper-danger); }
.co-btn--danger:hover:not(:disabled) { background:color-mix(in srgb,var(--copper-danger) 18%,var(--copper-surface)); }
</style>
