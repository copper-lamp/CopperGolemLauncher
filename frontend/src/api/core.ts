// 内核命令统一封装：invoke 包装 + 错误归一化。
//
// 后端命令均返回 `CommandResult<T>`；出错时 payload 为
// `{ kind: string, message: string }`（见 Rust `CommandError`），
// 这里统一抛 `KernelApiError`，业务层捕获后做 Toast / 弹窗反馈。

import { invoke } from "@tauri-apps/api/core";

/** 后端命令错误（与 Rust `CommandError` 同构）。 */
export interface CommandErrorPayload {
  kind: string;
  message: string;
}

/** 命令调用错误。 */
export class KernelApiError extends Error {
  readonly kind: string;

  constructor(payload: CommandErrorPayload) {
    super(payload.message);
    this.name = "KernelApiError";
    this.kind = payload.kind;
  }
}

/** 判断是否为后端命令错误。 */
export function isKernelApiError(e: unknown): e is KernelApiError {
  return e instanceof KernelApiError;
}

/** 调用内核命令，统一错误归一化为 `KernelApiError`。 */
export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    if (typeof e === "object" && e !== null && "kind" in e && "message" in e) {
      throw new KernelApiError(e as unknown as CommandErrorPayload);
    }
    // invoke 自身失败（如后端未就绪 / 参数错误）时兜底。
    throw new KernelApiError({
      kind: "invoke",
      message: typeof e === "string" ? e : String(e),
    });
  }
}
