// Toast 轻提示：全局单例，业务代码经 `useToast().show(...)` 触发。

import { reactive } from "vue";

export type ToastKind = "info" | "success" | "error";

/** Toast 上的可选操作按钮（如「立即重启」）。 */
export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface ToastItem {
  id: number;
  kind: ToastKind;
  message: string;
  /** 带操作按钮时，点击条身不再关闭（避免误触丢掉入口）。 */
  action?: ToastAction;
}

interface ToastState {
  items: ToastItem[];
}

const state = reactive<ToastState>({ items: [] });
let nextId = 1;

/** 显示一条轻提示，自动消失。 */
export function showToast(message: string, kind: ToastKind = "info", durationMs = 2600) {
  const id = nextId++;
  state.items.push({ id, kind, message });
  setTimeout(() => dismissToast(id), durationMs);
}

export interface ActionToastOptions {
  kind?: ToastKind;
  durationMs?: number;
  action?: ToastAction;
}

/**
 * 显示带操作按钮的提示。
 *
 * 默认停留更久（10s）：这类提示承载的是**唯一的后续入口**（例如「更新已就绪 →
 * 立即重启」），短提示一闪而过就等于没提示。
 */
export function showActionToast(message: string, options: ActionToastOptions = {}) {
  const { kind = "info", durationMs = 10_000, action } = options;
  const id = nextId++;
  state.items.push({ id, kind, message, action });
  setTimeout(() => dismissToast(id), durationMs);
}

/** 立即移除一条提示。 */
export function dismissToast(id: number) {
  const index = state.items.findIndex((i) => i.id === id);
  if (index >= 0) state.items.splice(index, 1);
}

/** 可组合式入口。 */
export function useToast() {
  return {
    items: state.items,
    info: (message: string) => showToast(message, "info"),
    success: (message: string) => showToast(message, "success"),
    error: (message: string) => showToast(message, "error"),
    action: showActionToast,
    dismiss: dismissToast,
  };
}