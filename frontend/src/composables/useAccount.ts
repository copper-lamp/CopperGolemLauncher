// 账户状态：当前账户 + 登录流程状态机。
//
// 登录流程：waiting（浏览器授权中）→ done / failed，状态经
// `account.login.state` 事件驱动；账户本体经 `account.changed` 更新。

import { readonly, ref } from "vue";

import {
  accountBeginLogin,
  accountCurrent,
  accountLogout,
  type AccountInfo,
} from "../api/account";
import { onAccountChanged, onAccountLoginState } from "../events";
import { useI18n } from "../i18n";
import { showToast } from "./useToast";

const account = ref<AccountInfo | null>(null);
const loginWaiting = ref(false);

/** 初始化：拉取当前账户并订阅事件。 */
export async function initAccount(): Promise<void> {
  try {
    account.value = await accountCurrent();
  } catch {
    // 内核未就绪时视为未登录。
  }
  await Promise.all([
    onAccountChanged((a) => {
      account.value = a;
    }),
    onAccountLoginState((state, reason) => {
      if (state === "waiting") {
        loginWaiting.value = true;
      } else {
        loginWaiting.value = false;
        if (state === "failed") {
          showToast(reason ?? "login failed", "error");
        }
      }
    }),
  ]);
}

export function useAccount() {
  const { t } = useI18n();

  async function beginLogin() {
    if (loginWaiting.value) return;
    try {
      const info = await accountBeginLogin();
      loginWaiting.value = true;
      // 设备码流程：浏览器已自动打开授权页，等待事件驱动完成。
      void info;
      showToast(t("account.login_begin_hint"), "info", 6000);
    } catch (e) {
      showToast(String(e), "error");
    }
  }

  async function logout() {
    try {
      await accountLogout();
      showToast(t("common.success"), "success");
    } catch (e) {
      showToast(String(e), "error");
    }
  }

  return {
    account: readonly(account),
    loginWaiting: readonly(loginWaiting),
    beginLogin,
    logout,
  };
}
