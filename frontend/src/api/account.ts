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
  /** WAM 的 WebAccount.Id（PUID）；只用于取票，不参与 profile 比对。 */
  wam_account_id?: string | null;
  /** Xbox 档案头像 URL；未取到时为 null，界面回退成 gamertag 首字母。 */
  avatar_url?: string | null;
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

/** Microsoft 账户授权（WAM）：弹出系统账户界面，返回选定账户身份。 */
export function accountWamSignIn(): Promise<AccountInfo> {
  return call<AccountInfo>("account_wam_sign_in");
}

/** 退出登录。 */
export function accountLogout(): Promise<void> {
  return call<void>("account_logout");
}

/**
 * 补齐账户头像：内核会在缺少头像时重新换票取一次（老账户行没有头像）。
 *
 * 取不到返回 `null`，调用方继续用首字母兜底，不要当成错误弹提示。
 */
export function accountRefreshAvatar(): Promise<string | null> {
  return call<string | null>("account_refresh_avatar");
}

/** 刷新当前账户令牌。 */
export function accountRefresh(): Promise<void> {
  return call<void>("account_refresh");
}

/** 取当前账户的 MSA + XSTS 凭证（供启动游戏使用）。 */
export function accountCredentials(): Promise<JsonValue> {
  return call<JsonValue>("account_credentials");
}
