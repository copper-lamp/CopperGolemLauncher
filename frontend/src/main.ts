// 应用入口：装配样式、路由、i18n、主题与全局状态。

import { createApp } from "vue";

import App from "./App.vue";
import router from "./router";

import "./styles/tokens.css";
import "./styles/base.css";

import { initI18n } from "./i18n";
import { initTheme } from "./theme";
import { initSettings } from "./composables/useSettings";
import { initDownloads } from "./composables/useDownloads";
import { initAccount } from "./composables/useAccount";
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
app.mount("#app");

// 内核能力初始化：与界面渲染并行，逐个失败互不影响。
void Promise.all([
  initI18n(),
  initTheme(),
  initSettings(),
  initDownloads(),
  initAccount(),
]);
