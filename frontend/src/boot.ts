// 启动引导运行时：记录每个引导步骤的进度与失败，并汇总启动期错误。
//
// 存在理由（一次真实的安卓现场）：
// 启动失败时，前端此前**没有任何可见出口** —— 加载页永远停在“进行中”，按键无响应，
// 用户除了“点不动”之外给不出任何信息，开发者也只能靠猜。根因是引导链被设计成
// 「所有能力都成功才放行」：任何一个 IPC 调用不返回，`markKernelReady` 就永远不执行。
//
// 因此这里做两件事，两者缺一不可：
// 1. **不再把 UI 的可见性押在 IPC 上**：引导改成有超时的多步流程，UI 一定渲染出来
//    （见 `main.ts`）；
// 2. **失败可见、可复制**：步骤表 + 启动期错误汇总，由 `DiagnosticOverlay.vue`
//    呈现给用户，指导其反馈问题。

import { readonly, ref } from "vue";

/** 单个引导步骤的状态。 */
export type BootStepState = "running" | "ok" | "failed" | "timeout";

/** 单个引导步骤的记录。 */
export interface BootStep {
  /** 稳定 id（报告里用它对齐，展示文案走 i18n）。 */
  id: string;
  /** i18n 键（`boot.error.step.*`）。 */
  labelKey: string;
  state: BootStepState;
  /** 开始时刻（相对页面加载，毫秒）。 */
  startedAt: number;
  /** 结束时刻；进行中为 null。 */
  endedAt: number | null;
  /** 失败原因；仅 `failed` / `timeout` 有值。 */
  error?: string;
}

/** 启动期收集到的原始事件（错误 / 警告 / 关键节点）。 */
export interface BootLogEntry {
  at: number;
  level: "info" | "warn" | "error";
  message: string;
}

/** 内核（Tauri IPC）可用性探测结果。 */
export type IpcState = "unknown" | "ready" | "unavailable";

const steps = ref<BootStep[]>([]);
const logEntries = ref<BootLogEntry[]>([]);
const ipcState = ref<IpcState>("unknown");

/** 日志条数上限：报告要能一眼看完，且不允许无界增长。 */
const MAX_LOG_ENTRIES = 120;

const now = (): number => Math.round(performance.now());

function findStep(id: string): BootStep | undefined {
  return steps.value.find((s) => s.id === id);
}

/**
 * 登记一个引导步骤为「进行中」。
 *
 * 步骤由引导代码显式登记，而不是自动埋点：报告要能直接回答
 * 「哪一步没回来」，就必须先有明确的步骤边界。
 */
export function beginStep(id: string, labelKey: string): void {
  const existing = findStep(id);
  if (existing) {
    existing.state = "running";
    existing.startedAt = now();
    existing.endedAt = null;
    delete existing.error;
    return;
  }
  steps.value = [
    ...steps.value,
    { id, labelKey, state: "running", startedAt: now(), endedAt: null },
  ];
}

/** 标记步骤成功。 */
export function endStep(id: string): void {
  const step = findStep(id);
  if (!step) return;
  step.state = "ok";
  step.endedAt = now();
}

/** 标记步骤失败（业务失败：后端明确报错）。 */
export function failStep(id: string, error: unknown): void {
  const step = findStep(id);
  if (!step) return;
  step.state = "failed";
  step.endedAt = now();
  step.error = describeError(error);
}

/**
 * 标记步骤超时（更危险的一类失败：调用根本没回来）。
 *
 * 与 `failed` 分开：超时说明 IPC 链路本身可能有问题，处理方式不同
 * ——失败可以继续用降级数据，超时必须显式提示并给出重试入口。
 */
export function timeoutStep(id: string, ms: number): void {
  const step = findStep(id);
  if (!step) return;
  step.state = "timeout";
  step.endedAt = now();
  step.error = `timeout after ${ms}ms`;
}

/** 追加一条启动期日志。 */
export function logBoot(level: BootLogEntry["level"], message: string): void {
  const next = [...logEntries.value, { at: now(), level, message }];
  logEntries.value = next.length > MAX_LOG_ENTRIES ? next.slice(-MAX_LOG_ENTRIES) : next;
}

/** 设置内核可用性探测结果。 */
export function setIpcState(state: IpcState): void {
  ipcState.value = state;
}

/** 把一个异常压成单行可读文本。 */
export function describeError(error: unknown): string {
  if (error instanceof Error) return error.message || error.name;
  if (typeof error === "string") return error;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

/** 启动期诊断状态（响应式，只读语义：外部经上面的函数写入）。 */
export function useBootDiagnostics() {
  return {
    steps: readonly(steps),
    logEntries: readonly(logEntries),
    ipcState: readonly(ipcState),
  };
}

/**
 * 生成可直接粘贴给开发者的诊断报告。
 *
 * 刻意做成纯文本：用户截屏会丢信息，纯文本可以整段复制进 issue。
 */
export function bootReport(): string {
  const lines: string[] = [];
  lines.push(`copper-golem boot report`);
  lines.push(`url: ${location.href}`);
  lines.push(`ua: ${navigator.userAgent}`);
  lines.push(`platform: ${navigator.platform ?? "unknown"}`);
  lines.push(`viewport: ${window.innerWidth}x${window.innerHeight} @${window.devicePixelRatio}`);
  lines.push(`protocol: ${location.protocol}`);
  lines.push(`ipc: ${ipcState.value}`);
  lines.push(`tauri-globals: internals=${hasTauriInternals()} ipc=${hasIpcBridge()}`);
  lines.push("");
  lines.push("steps:");
  for (const step of steps.value) {
    const cost = step.endedAt === null ? "…" : `${step.endedAt - step.startedAt}ms`;
    lines.push(
      `  ${step.id.padEnd(16)} ${step.state.padEnd(8)} ${cost.padStart(8)}` +
        (step.error ? `  ${step.error}` : ""),
    );
  }
  lines.push("");
  lines.push("log:");
  for (const entry of logEntries.value) {
    lines.push(`  [+${String(entry.at).padStart(7)}ms][${entry.level}] ${entry.message}`);
  }
  return lines.join("\n");
}

/** `window.__TAURI_INTERNALS__` 是否已注入（Tauri 初始化脚本执行过）。 */
export function hasTauriInternals(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * Tauri 注入的运行时桥。
 *
 * 与 `@tauri-apps/api` 内部的用法保持同一形状（`ipc` / `invoke`），但这里只声明
 * 诊断真正会碰的两个成员：模块不该假装知道宿主脚本的全部接口。
 */
interface TauriInternals {
  ipc?: unknown;
  invoke?: (cmd: string, args?: unknown) => Promise<unknown>;
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: TauriInternals;
  }
}

/**
 * Tauri 的 `ipc` 桥是否就绪。
 *
 * `@tauri-apps/api` 的 `invoke` 在桥缺失时**既不 resolve 也不 reject**，而是每 50ms
 * 轮询等待（见 Tauri 的 `scripts/core.js`）。这就是「页面永远加载中、点不动」的
 * 直接机制，所以引导流程必须主动探测它，而不是等调用返回。
 */
export function hasIpcBridge(): boolean {
  return typeof window.__TAURI_INTERNALS__?.ipc === "function";
}

/**
 * 在页面加载后立刻安装错误采集。
 *
 * 必须早于任何其它模块执行（`main.ts` 顶部调用）：引导期抛出的异常往往发生在
 * 首帧渲染之前，晚装一步就只剩「白屏 / 卡加载」这一个现象。
 */
export function installBootDiagnostics(): void {
  window.addEventListener("error", (event) => {
    const target = event.target as HTMLElement | null;
    // 资源加载失败（script / link / img）不走 message 字段，需单独识别。
    if (event.message) {
      logBoot("error", `uncaught: ${event.message} @ ${event.filename}:${event.lineno}:${event.colno}`);
    } else if (target) {
      const url = (target as HTMLScriptElement).src ?? (target as HTMLLinkElement).href ?? "";
      logBoot("error", `resource failed: <${target.tagName.toLowerCase()}> ${url}`);
    } else {
      logBoot("error", "unknown error event");
    }
    forward(lastError());
  });

  window.addEventListener("unhandledrejection", (event) => {
    const message = describeError(event.reason);
    logBoot("error", `unhandledrejection: ${message}`);
    forward(message);
  });
}

let lastMessage = "";

function lastError(): string {
  const entries = logEntries.value;
  const last = entries.length > 0 ? entries[entries.length - 1] : undefined;
  return last?.message ?? "";
}

/**
 * 把诊断文本转发到内核日志（失败静默）。
 *
 * 直连 `window.__TAURI_INTERNALS__.invoke` 而不是 `@tauri-apps/api`：桥缺失时
 * API 会永远挂起，而这里只希望「能记就记，记不上就算了」。
 */
export function forward(message: string): void {
  if (!message || !hasIpcBridge()) return;
  if (lastMessage === message) return;
  lastMessage = message;
  try {
    void window.__TAURI_INTERNALS__?.invoke?.("debug_log", { level: "error", message })?.catch?.(
      () => {},
    );
  } catch {
    // 转发失败不影响用户可见的诊断面板。
  }
}
