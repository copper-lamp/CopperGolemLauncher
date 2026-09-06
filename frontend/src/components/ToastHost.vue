<script setup lang="ts">
// Toast 宿主：渲染全局轻提示（顶部居中，自动消失）。

import { useToast } from "../composables/useToast";
import { CheckCircle2, AlertTriangle, Info } from "@lucide/vue";

const { items, dismiss } = useToast();

function iconFor(kind: string) {
  switch (kind) {
    case "success":
      return { icon: CheckCircle2, cls: "is-success" };
    case "error":
      return { icon: AlertTriangle, cls: "is-error" };
    default:
      return { icon: Info, cls: "is-info" };
  }
}
</script>

<template>
  <Teleport to="body">
    <div class="toast-host">
      <TransitionGroup name="toast">
        <div v-for="item in items" :key="item.id" class="toast" @click="dismiss(item.id)">
          <component
            :is="iconFor(item.kind).icon"
            :size="15"
            class="toast__icon"
            :class="iconFor(item.kind).cls"
          />
          <span class="toast__msg">{{ item.message }}</span>
        </div>
      </TransitionGroup>
    </div>
  </Teleport>
</template>

<style scoped>
.toast-host {
  position: fixed;
  top: 52px;
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-2);
  z-index: 1000;
  pointer-events: none;
}

.toast {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  max-width: 420px;
  padding: var(--copper-space-2) var(--copper-space-4);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  box-shadow: var(--copper-shadow);
  font-size: var(--copper-font-size-sm);
  pointer-events: auto;
  cursor: pointer;
}

.toast__icon {
  flex-shrink: 0;
}

.toast__icon.is-success {
  color: var(--copper-success);
}

.toast__icon.is-error {
  color: var(--copper-danger);
}

.toast__icon.is-info {
  color: var(--copper-accent);
}

.toast__msg {
  overflow-wrap: anywhere;
}

.toast-enter-active,
.toast-leave-active {
  transition: all var(--copper-duration) var(--copper-easing);
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateY(-8px);
}
</style>
