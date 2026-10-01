// 启动诊断面板的开关状态。
//
// 单独成模块的原因与 `useKernelReady` 一致：写入方是引导流程（`main.ts`），
// 读取方是 Shell（`App.vue`）与面板本身，三方不应互相 import。
//
// 面板的两种出现方式：
// 1. **引导失败或超时**：自动弹出。用户此前只能看到「一直加载中」，
//    现在至少能拿到一段可复制的报告；
// 2. **`?diag=1`**：用户或开发者主动打开，用于「界面能进但有异常」的现场。

import { ref } from "vue";

import { useBootDiagnostics } from "./boot";

/** 面板是否展开（响应式，只读语义：外部经下面的函数写入）。 */
export const diagnosticOpen = ref(false);

/** 是否由引导失败自动弹出（自动弹出时文案更强调「出问题了」）。 */
export const diagnosticAutoOpened = ref(false);

/** 用户主动打开 / 收起面板。 */
export function setDiagnosticOpen(value: boolean): void {
  diagnosticOpen.value = value;
  if (!value) diagnosticAutoOpened.value = false;
}

/** 引导失败时自动展开面板（幂等：已展开即不再改动来源标记）。 */
export function openDiagnosticOnFailure(): void {
  if (diagnosticOpen.value) return;
  diagnosticOpen.value = true;
  diagnosticAutoOpened.value = true;
}

/**
 * 判断引导是否出现需要用户知道的失败。
 *
 * 只把「失败 / 超时」算作异常：`running` 是正常中间态，不能因为引导还在跑
 * 就弹报错面板 —— 那会把正常的慢启动变成一次惊吓。
 */
export function hasBootFailure(): boolean {
  const { steps } = useBootDiagnostics();
  return steps.value.some((step) => step.state === "failed" || step.state === "timeout");
}
