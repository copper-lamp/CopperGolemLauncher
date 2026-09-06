<script setup lang="ts">
// 通用按钮：primary / secondary / ghost / danger 四型。

withDefaults(
  defineProps<{
    variant?: "primary" | "secondary" | "ghost" | "danger";
    size?: "md" | "sm";
    disabled?: boolean;
    type?: "button" | "submit";
  }>(),
  {
    variant: "secondary",
    size: "md",
    disabled: false,
    type: "button",
  },
);

const emit = defineEmits<{ click: [event: MouseEvent] }>();
</script>

<template>
  <button
    :type="type"
    :disabled="disabled"
    :class="['co-btn', `co-btn--${variant}`, `co-btn--${size}`]"
    @click="emit('click', $event)"
  >
    <slot />
  </button>
</template>

<style scoped>
.co-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-2);
  border: 1px solid transparent;
  border-radius: var(--copper-radius-md);
  font-weight: 500;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
  white-space: nowrap;
}

.co-btn--md {
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-4);
  font-size: var(--copper-font-size-md);
}

.co-btn--sm {
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-3);
  font-size: var(--copper-font-size-sm);
}

.co-btn:active:not(:disabled) {
  transform: scale(0.97);
}

.co-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.co-btn--primary {
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
}

.co-btn--primary:hover:not(:disabled) {
  filter: brightness(1.06);
}

.co-btn--secondary {
  background: var(--copper-surface-2);
  color: var(--copper-text);
  border-color: var(--copper-border);
}

.co-btn--secondary:hover:not(:disabled) {
  background: var(--copper-surface-3);
}

.co-btn--ghost {
  background: transparent;
  color: var(--copper-text-secondary);
}

.co-btn--ghost:hover:not(:disabled) {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.co-btn--danger {
  background: transparent;
  color: var(--copper-danger);
  border-color: color-mix(in srgb, var(--copper-danger) 40%, transparent);
}

.co-btn--danger:hover:not(:disabled) {
  background: color-mix(in srgb, var(--copper-danger) 12%, transparent);
}
</style>
