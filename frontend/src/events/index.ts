// 事件监听封装：内核事件总线的同名事件自动桥接到前端
// （见 Rust `EventBus::publish` 的 `app.emit`），这里提供类型化订阅入口。

import { listen } from "@tauri-apps/api/event";

import type { DownloadTask } from "../api/download";
import type { AccountInfo, LoginState } from "../api/account";
import type { UpdateStatus } from "../api/updater";
import type { JsonValue } from "../api/types";

/** 订阅取消函数。 */
export type Unlisten = () => void;

/** 订阅下载事件（created / progress / status）。 */
export function onDownload(
  event: "created" | "progress" | "status",
  handler: (task: DownloadTask) => void,
): Promise<Unlisten> {
  return listen<DownloadTask>(`download.${event}`, (e) => handler(e.payload));
}

/** 订阅账户登录流程状态。 */
export function onAccountLoginState(
  handler: (state: LoginState, reason: string | null) => void,
): Promise<Unlisten> {
  return listen<{ state: LoginState; reason: string | null }>(
    "account.login.state",
    (e) => handler(e.payload.state, e.payload.reason),
  );
}

/** 订阅账户变化。 */
export function onAccountChanged(
  handler: (account: AccountInfo | null) => void,
): Promise<Unlisten> {
  return listen<{ account: AccountInfo | null }>("account.changed", (e) =>
    handler(e.payload.account),
  );
}

/** 订阅设置变更（负载为发生变更的键值集合）。 */
export function onSettingsChanged(
  handler: (changed: Record<string, JsonValue>) => void,
): Promise<Unlisten> {
  return listen<Record<string, JsonValue>>("settings.changed", (e) =>
    handler(e.payload),
  );
}

/** 订阅版本安装事件（游戏下载模块发布，开始页据此刷新清单）。 */
export function onVersionInstalled(
  handler: (name: string) => void,
): Promise<Unlisten> {
  return listen<{ name: string }>("version.installed", (e) =>
    handler(e.payload.name),
  );
}

/** 订阅版本删除事件（开始页据此刷新清单）。 */
export function onVersionRemoved(
  handler: (name: string) => void,
): Promise<Unlisten> {
  return listen<{ name: string }>("version.removed", (e) =>
    handler(e.payload.name),
  );
}

/** 订阅更新状态。 */
export function onUpdateStatus(handler: (status: UpdateStatus) => void): Promise<Unlisten> {
  return listen<UpdateStatus>("update.status", (e) => handler(e.payload));
}
