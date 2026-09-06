// Toast 轻提示：全局单例，业务代码经 `useToast().show(...)` 触发。

import { reactive } from "vue";

export type ToastKind = "info" | "success" | "error";

export interface ToastItem {
  id: number;
  kind: ToastKind;
  message: string;
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
    dismiss: dismissToast,
  };
}
