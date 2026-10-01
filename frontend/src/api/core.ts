// 内核命令统一封装：invoke 包装 + 错误归一化 + 无响应保护。
//
// 后端命令均返回 `CommandResult<T>`；出错时 payload 为
// `{ kind: string, message: string }`（见 Rust `CommandError`），
// 这里统一抛 `KernelApiError`，业务层捕获后做 Toast / 弹窗反馈。
//
// # 为什么必须有无响应保护（安卓卡加载事故的直接结论）
//
// Tauri 的 `invoke` 在 IPC 桥缺失时既不 resolve 也不 reject（见 `boot.ts`），
// 而业务代码普遍写成「await 成功才把 loading 置回 false」。两者相叠的结果就是
// 界面永远停在加载态、点哪都没反应，且**没有任何错误冒出来**。
// 因此这里给每个调用加默认超时：无响应必须变成一条明确错误，而不是无限等待。
//
// 超时值按「内核真实耗时」分级，而不是一律取最小值：
// - 普通命令（读设置 / 列表 / 状态查询）等待时间远小于一秒，默认上限足够宽裕；
// - 网络、解包、安装等命令必须显式声明更长的预算（见 `LONG_RUNNING_COMMANDS`），
//   否则会把正常的长任务误判为无响应。

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

/** 默认无响应上限：普通命令够用，且远早于用户失去耐心。 */
const DEFAULT_TIMEOUT_MS = 30_000;

/**
 * 长任务命令的显式预算（毫秒）。
 *
 * 这些命令会联网、解包或跑安装流水线，耗时以分钟计：给它们 30 秒的默认值
 * 会把正常任务误报成「无响应」。名单**必须**随新增长任务命令同步，
 * 因此这里只登记真正会长时间阻塞的那几个，而不是把所有命令都放宽。
 */
const LONG_RUNNING_COMMANDS: Record<string, number> = {
  // 商店 / 微软账户往返（含设备注册与重定向）。
  account_begin_login: 180_000,
  account_wam_sign_in: 180_000,
  account_refresh: 120_000,
  // 安装流水线（校验整包、解包、注册版本）与整包重装。
  game_download_install: 900_000,
  game_download_import_apk: 900_000,
  // 下载与安装共用的长任务投递。
  download_enqueue: 120_000,
  // 内容下载：远端清单 / 详情 / 文档 / 环境探测（均带网络往返）。
  content_download_list: 120_000,
  content_download_detail: 120_000,
  content_download_readme: 120_000,
  content_download_game_versions: 120_000,
  content_download_download: 120_000,
  content_download_lip_env: 300_000,
  content_download_lip_install: 600_000,
  // 游戏启动：准备运行时并唤起游戏进程 / Activity。
  home_launch: 300_000,
  // 元数据源刷新（索引 + 分片下载与校验）。
  registry_refresh: 300_000,
  // 更新检查与安装。
  updater_check: 120_000,
  updater_apply: 900_000,
  updater_install: 900_000,
  // 附加模块安装（下载 + 解包 + 校验）。
  modules_install: 600_000,
  // 附加模块后端调用：模块自定义命令耗时未知，按长任务处理。
  module_invoke: 600_000,
};

/** 单次调用的可选项。 */
export interface CallOptions {
  /** 覆盖无响应上限（毫秒）。 */
  timeoutMs?: number;
}

/** 该命令的无响应上限。 */
function timeoutFor(cmd: string, overrides?: CallOptions): number {
  if (overrides?.timeoutMs !== undefined) return overrides.timeoutMs;
  return LONG_RUNNING_COMMANDS[cmd] ?? DEFAULT_TIMEOUT_MS;
}

/**
 * 调用内核命令，统一错误归一化为 `KernelApiError`。
 *
 * 两种失败都归一到同一个错误类型，业务层不必分别处理：
 * - 后端明确报错 → 原样保留 `kind` / `message`；
 * - 超时无响应 → `kind: "timeout"`，消息里带上命令名与等待时长，
 *   便于用户反馈时直接说明是哪一步卡住。
 */
export async function call<T>(
  cmd: string,
  args?: Record<string, unknown>,
  options?: CallOptions,
): Promise<T> {
  const budget = timeoutFor(cmd, options);
  let timer = 0;
  const timeout = new Promise<never>((_, reject) => {
    timer = window.setTimeout(() => {
      reject(
        new KernelApiError({
          kind: "timeout",
          message: `内核命令 ${cmd} 在 ${budget}ms 内没有响应`,
        }),
      );
    }, budget);
  });

  try {
    return await Promise.race([invoke<T>(cmd, args), timeout]);
  } catch (e) {
    if (e instanceof KernelApiError) throw e;
    if (typeof e === "object" && e !== null && "kind" in e && "message" in e) {
      throw new KernelApiError(e as unknown as CommandErrorPayload);
    }
    // invoke 自身失败（如后端未就绪 / 参数错误）时兜底。
    throw new KernelApiError({
      kind: "invoke",
      message: typeof e === "string" ? e : String(e),
    });
  } finally {
    // 命令成功返回后必须清掉定时器：否则它会在后台把进程多留 30 秒，
    // 移动端还可能因此被系统判定为后台活跃。
    window.clearTimeout(timer);
  }
}
