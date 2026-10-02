<script setup lang="ts">
// Toast 宿主：渲染全局轻提示（顶部居中，自动消失）。

import { useToast } from "../composables/useToast";
import { CheckCircle2, AlertTriangle, Info } from "@lucide/vue";

const { items, dismiss } = useToast();

/** 执行操作按钮：先移除提示再执行，避免回调失败时提示还挂在屏幕上。 */
function runAction(item: { id: number; action?: { onClick: () => void } }) {
  const action = item.action;
  if (!action) return;
  dismiss(item.id);
  action.onClick();
}

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
        <div
          v-for="item in items"
          :key="item.id"
          class="toast"
          :class="{ 'toast--action': item.action }"
          @click="!item.action && dismiss(item.id)"
        >
          <component
            :is="iconFor(item.kind).icon"
            :size="15"
            class="toast__icon"
            :class="iconFor(item.kind).cls"
          />
          <span class="toast__msg">{{ item.message }}</span>
          <!-- 带操作入口的提示：条身点击不再关闭，否则用户点一下就把入口丢了。 -->
          <button
            v-if="item.action"
            class="toast__action"
            @click.stop="runAction(item)"
          >
            {{ item.action.label }}
          </button>
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

/* 带操作按钮时条身不可点（避免与按钮抢点击），光标退化为默认。 */
.toast--action {
  cursor: default;
  border-color: color-mix(in srgb, var(--copper-accent) 35%, var(--copper-border));
}

.toast__action {
  flex-shrink: 0;
  padding: 0 var(--copper-space-3);
  height: 24px;
  border: 1px solid color-mix(in srgb, var(--copper-accent) 45%, transparent);
  border-radius: var(--copper-radius-sm);
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
  color: var(--copper-accent);
  font-family: inherit;
  font-size: var(--copper-font-size-xs);
  font-weight: 600;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.toast__action:hover {
  background: color-mix(in srgb, var(--copper-accent) 24%, transparent);
  border-color: var(--copper-accent);
}

.toast__action:active {
  transform: scale(0.96);
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
