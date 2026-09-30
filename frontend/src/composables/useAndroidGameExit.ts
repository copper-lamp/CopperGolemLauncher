// 安卓游戏退出观测。
//
// 退出检测由 Java 宿主完成（`com.mojang.minecraftpe` 的原生库不可卸载，
// 也没有能穿透到 Tauri 内核的 Java 回调），宿主把退出记录写进文件信箱，
// 这里负责取走并广播。取走语义（读后即删）保证同一次退出只被消费一次。
//
// 触发时机：应用挂载后一次，以及窗口重新获得焦点时一次——安卓上从游戏
// 返回启动器必然触发 focus，因此不需要在后台做轮询。
import { ref } from "vue";

import { invoke } from "@tauri-apps/api/core";

import { androidGameTakeExit, type AndroidGameExit } from "../modules/game-download/api";

/** 最近一次游戏退出记录；无退出时为 `null`。 */
export const lastAndroidGameExit = ref<AndroidGameExit | null>(null);

function forward(detail: string, level: "info" | "error" = "info"): void {
  void invoke("debug_log", { level, message: detail }).catch(() => {});
}

/** 取走一次退出记录；无记录时静默返回。 */
export async function drainAndroidGameExit(): Promise<void> {
  try {
    const record = await androidGameTakeExit();
    if (!record) return;
    lastAndroidGameExit.value = record;
    forward(`Android game exited: ${record.instance_name} (${record.reason})`);
  } catch (error) {
    forward(`读取安卓游戏退出记录失败: ${String(error)}`, "error");
  }
}

/** 挂载观测：注册焦点监听并立即排空一次。 */
export function observeAndroidGameExit(): void {
  void drainAndroidGameExit();
  window.addEventListener("focus", () => void drainAndroidGameExit());
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") void drainAndroidGameExit();
  });
}
