// 账户 API：当前账户、发起登录、退出登录、刷新、取启动凭证。
//
// 登录流程状态经 `account.login.state` 事件推送；账户变化经 `account.changed`。

import type { JsonValue } from "./types";
import { call } from "./core";

/** 账户公开信息。 */
export interface AccountInfo {
  id: string;
  gamertag: string;
  xuid: string | null;
}

/** 设备码信息（前端展示授权链接与用户码）。 */
export interface DeviceCodeInfo {
  device_code: string;
  user_code: string;
  verification_uri: string;
  message: string;
  expires_in_sec: number;
}

/** 登录流程状态。 */
export type LoginState = "waiting" | "done" | "failed";

/** 当前登录账户。 */
export function accountCurrent(): Promise<AccountInfo | null> {
  return call<AccountInfo | null>("account_current");
}

/** 发起设备码登录：返回授权信息，后台自动轮询。 */
export function accountBeginLogin(): Promise<DeviceCodeInfo> {
  return call<DeviceCodeInfo>("account_begin_login");
}

/** 退出登录。 */
export function accountLogout(): Promise<void> {
  return call<void>("account_logout");
}

/** 刷新当前账户令牌。 */
export function accountRefresh(): Promise<void> {
  return call<void>("account_refresh");
}

/** 取当前账户的 MSA + XSTS 凭证（供启动游戏使用）。 */
export function accountCredentials(): Promise<JsonValue> {
  return call<JsonValue>("account_credentials");
}
