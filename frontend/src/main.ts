// 应用入口：装配样式、路由、i18n、主题与全局状态。

import { createApp } from "vue";

import App from "./App.vue";
import router from "./router";

import "./styles/tokens.css";
import "./styles/base.css";

import { initI18n } from "./i18n";
import { initTheme } from "./theme";
import { initPlatform } from "./composables/usePlatform";
import { initSettings } from "./composables/useSettings";
import { initDownloads } from "./composables/useDownloads";
import { initAccount } from "./composables/useAccount";
import { markKernelReady } from "./composables/useKernelReady";
import { loadAddonFrontends } from "./modules/addonRuntime";
import { invoke } from "@tauri-apps/api/core";

// 临时诊断：把前端 JS 运行错误 / 未处理 rejection 转发到内核日志，便于定位白屏。
function forward(detail: string) {
  void invoke("debug_log", { level: "error", message: detail }).catch(() => {});
}
window.addEventListener("error", (e) => {
  forward(`uncaught: ${e.message}\nURL=${e.filename}:${e.lineno}:${e.colno}`);
});
window.addEventListener("unhandledrejection", (e) => {
  const reason = e.reason;
  forward(
    `unhandledrejection: ${
      typeof reason === "string" ? reason : reason?.message ?? String(reason)
    }`,
  );
});

const app = createApp(App);
app.use(router);

// 附加模块前端必须在挂载**之前**登记：左导航与路由表在首次渲染时被读取，
// 挂载后再追加会出现"导航项在、首次点击 404"的竞态。
// 单个模块加载失败只影响它自己，不阻断内置界面（见 addonRuntime 的错误隔离）。
void Promise.race([
  loadAddonFrontends(router),
  new Promise<never>((_, reject) =>
    window.setTimeout(() => reject(new Error("附加模块前端加载超时")), 3000),
  ),
])
  .catch((error) => {
    forward(`addon frontend bootstrap failed: ${String(error)}`);
  })
  .finally(() => {
    app.mount("#app");
  });

void Promise.allSettled([initSettings(), initDownloads(), initAccount()]);

void Promise.all([initI18n(), initTheme(), initPlatform()]).finally(() => {
  markKernelReady();
});
