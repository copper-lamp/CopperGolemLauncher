// 事件监听封装：内核事件总线的同名事件自动桥接到前端
// （见 Rust `EventBus::publish` 的 `app.emit`），这里提供类型化订阅入口。

import { listen } from "@tauri-apps/api/event";

import type { DownloadTask } from "../api/download";
import type { AccountInfo, LoginState } from "../api/account";
import type { UpdateStatus } from "../api/updater";
import type { JsonValue } from "../api/types";

/** 订阅取消函数。 */
export type Unlisten = () => void;

export function onGameLaunched(handler: (payload: { name: string }) => void): Promise<Unlisten> {
  return listen<{ name: string }>("game-launched", (e) => handler(e.payload));
}

export function onGameExited(handler: (payload: { name: string; reason?: string }) => void): Promise<Unlisten> {
  return listen<{ name: string; reason?: string }>("game-exited", (e) => handler(e.payload));
}

/** 订阅下载事件（created / progress / status）。 */
export function onDownload(
  event: "created" | "progress" | "status",
  handler: (task: DownloadTask) => void,
): Promise<Unlisten> {
  return listen<DownloadTask>(`download-${event}`, (e) => handler(e.payload));
}

/** 订阅账户登录流程状态。 */
export function onAccountLoginState(
  handler: (state: LoginState, reason: string | null) => void,
): Promise<Unlisten> {
  return listen<{ state: LoginState; reason: string | null }>(
    "account-login-state",
    (e) => handler(e.payload.state, e.payload.reason),
  );
}

/** 订阅账户变化。 */
export function onAccountChanged(
  handler: (account: AccountInfo | null) => void,
): Promise<Unlisten> {
  return listen<{ account: AccountInfo | null }>("account-changed", (e) =>
    handler(e.payload.account),
  );
}

/** 订阅设置变更（负载为发生变更的键值集合）。 */
export function onSettingsChanged(
  handler: (changed: Record<string, JsonValue>) => void,
): Promise<Unlisten> {
  return listen<Record<string, JsonValue>>("settings-changed", (e) =>
    handler(e.payload),
  );
}

/** 订阅版本清单变更（版本根目录切换等，开始页据此刷新清单）。 */
export function onVersionsChanged(
  handler: (payload: { gameDirectory: string }) => void,
): Promise<Unlisten> {
  return listen<{ gameDirectory: string }>("versions-changed", (e) =>
    handler(e.payload),
  );
}

/** 订阅版本安装事件（游戏下载模块发布，开始页据此刷新清单）。 */
export function onVersionInstalled(
  handler: (name: string) => void,
): Promise<Unlisten> {
  return listen<{ name: string }>("version-installed", (e) =>
    handler(e.payload.name),
  );
}

/** 订阅版本删除事件（开始页据此刷新清单）。 */
export function onVersionRemoved(
  handler: (name: string) => void,
): Promise<Unlisten> {
  return listen<{ name: string }>("version-removed", (e) =>
    handler(e.payload.name),
  );
}

/** 订阅模组变更事件（导入 / 启停 / 删除 / 清单编辑后由后端广播）。 */
export function onModsChanged(
  handler: (name: string) => void,
): Promise<Unlisten> {
  return listen<{ name: string }>("mods-changed", (e) =>
    handler(e.payload.name),
  );
}

/** 游戏下载模块：任务已投递（负载 `{ id, instance, taskId }`）。 */
export function onGameDownloadEnqueued(
  handler: (payload: { id: string; instance: string; taskId: number }) => void,
): Promise<Unlisten> {
  return listen<{ id: string; instance: string; taskId: number }>(
    "game-download-enqueued",
    (e) => handler(e.payload),
  );
}

/** 游戏下载模块：一次实例安装完成（负载 `{ id, instance }`）。 */
export function onGameDownloadInstalled(
  handler: (payload: { id: string; instance: string }) => void,
): Promise<Unlisten> {
  return listen<{ id: string; instance: string }>("game-download-installed", (e) =>
    handler(e.payload),
  );
}

/** 游戏下载模块：任务失败（负载 `{ id, error }`）。 */
export function onGameDownloadFailed(
  handler: (payload: { id: string; error: string }) => void,
): Promise<Unlisten> {
  return listen<{ id: string; error: string }>("game-download-failed", (e) =>
    handler(e.payload),
  );
}

/** 游戏下载模块：安装被取消（负载 `{ id, instance? }`；`instance` 缺省表示整包放弃）。 */
export function onGameDownloadCancelled(
  handler: (payload: { id: string; instance?: string }) => void,
): Promise<Unlisten> {
  return listen<{ id: string; instance?: string }>("game-download-cancelled", (e) =>
    handler(e.payload),
  );
}

/** 内容下载落点事件负载。
 *
 * 三个时点共用一个事件（后端 `content_download::announce_placement` 与安装钩子）：
 * - `kind: "download_only"` —— 投递即发。内容没装进游戏，落到了系统下载目录；
 * - `kind: "install"` —— 投递即发，附内容根不可用等原因（`notice`）；
 * - `kind: "installed"` —— 落位成功后发，附目标版本与真实落点。
 */
export interface ContentDownloadLocation {
  /** 内容 id（`install` 阶段的纯提示不带）。 */
  id?: string;
  kind?: "download_only" | "install" | "installed";
  /** 目标实例名。 */
  version?: string | null;
  /** 落点目录：`download_only` 时是下载目录，`installed` 时是真实落点。 */
  dir?: string;
  /** 下载完成的文件路径（`download_only`）。 */
  dest?: string;
  /** 投递时即知的提示。 */
  notice?: string | null;
  /** 失败原因。 */
  error?: string;
}

/** 订阅内容下载落点（前端据此弹吐司）。 */
export function onContentDownloadLocation(
  handler: (payload: ContentDownloadLocation) => void,
): Promise<Unlisten> {
  return listen<ContentDownloadLocation>("content-download.location", (e) =>
    handler(e.payload),
  );
}

/** 订阅更新状态。 */
export function onUpdateStatus(handler: (status: UpdateStatus) => void): Promise<Unlisten> {
  return listen<UpdateStatus>("update-status", (e) => handler(e.payload));
}

/** 订阅「更新包已就绪」事件（后端在下载完成的瞬间额外发一次）。
 *
 * 单独一条通道的意义：全局提示层据此弹「立即重启」，
 * 不必去轮询下载任务，也不用在 `update-status` 里做相位推断。
 */
export function onUpdateReady(
  handler: (payload: { version: string; download_task_id: number }) => void,
): Promise<Unlisten> {
  return listen<{ version: string; download_task_id: number }>("update-ready", (e) =>
    handler(e.payload),
  );
}
