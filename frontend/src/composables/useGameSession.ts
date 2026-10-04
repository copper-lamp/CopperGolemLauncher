// 游戏会话状态：全局单例，跟踪「未启动 / 启动中 / 运行中」。
//
// 为什么放全局而不是 `HomePage` 的局部状态：按钮形态（启动 → 退出）取决于
// **游戏进程的真实状态**，而用户在游戏运行期间可以随意切换页面。把状态留在
// 开始页里，用户从设置页回来时按钮就会退回「启动」，进而允许对已在运行的
// 实例再启动一次。
//
// 状态的唯一权威来源是内核事件（`game.launched` / `game.exited`），本模块
// 只做转发；命令 `home_game_running` 用于进入页面时对齐一次真实进程状态
// （例如启动器重启后游戏仍在运行）。
import { ref, watch } from "vue";

import { homeGameRunning } from "../api/home";
import { onGameExited, onGameLaunched } from "../events";
import { lastAndroidGameExit } from "./useAndroidGameExit";

/** 会话状态。 */
export type GameSessionState = "idle" | "launching" | "running";

/** 当前状态。 */
export const gameSessionState = ref<GameSessionState>("idle");
/** 状态所属的实例名；`idle` 时为空串。 */
export const gameSessionName = ref("");

let started = false;

/** 进入「启动中」；同一时刻只允许一个启动流程。 */
export function beginLaunch(name: string): void {
  gameSessionName.value = name;
  gameSessionState.value = "launching";
}

/** 回到「未启动」。 */
export function endSession(): void {
  gameSessionState.value = "idle";
  gameSessionName.value = "";
}

/** 该实例当前是否处于启动中或运行中（用于按钮形态与禁用）。 */
export function isSessionBusy(name: string): boolean {
  return gameSessionState.value !== "idle" && gameSessionName.value === name;
}

/**
 * 与真实进程状态对齐一次。
 *
 * 只在「非启动中」时覆盖：启动确认窗口内进程可能还没起来，此时用一次
 * `false` 覆盖会把刚点下的启动直接判成失败。
 */
export async function syncSessionState(name: string): Promise<void> {
  if (!name || gameSessionState.value === "launching") return;
  try {
    const running = await homeGameRunning(name);
    if (running) {
      gameSessionName.value = name;
      gameSessionState.value = "running";
    } else if (gameSessionName.value === name) {
      endSession();
    }
  } catch {
    // 探测失败（版本目录不可用等）时保持现状，不打断界面。
  }
}

/** 订阅内核事件，挂载一次即可（重复调用无副作用）。 */
export async function initGameSession(): Promise<void> {
  if (started) return;
  started = true;

  await Promise.all([
    onGameLaunched(({ name }) => {
      gameSessionName.value = name;
      gameSessionState.value = "running";
    }),
    onGameExited(({ name }) => {
      // 只接受属于当前会话的退出，避免启动 A 时收到 B 的退出把状态清掉。
      if (gameSessionName.value === name) endSession();
    }),
  ]);

  // 安卓没有进程可轮询，退出经 Java 宿主的文件信箱回传（`lastAndroidGameExit`
  // 由 `observeAndroidGameExit` 排空后写入）。按实例名匹配，避免旧记录清掉新会话。
  watch(lastAndroidGameExit, (record) => {
    if (record && record.instance_name === gameSessionName.value) endSession();
  });
}
