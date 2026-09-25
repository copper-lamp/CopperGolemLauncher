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
void loadAddonFrontends(router).finally(() => {
  app.mount("#app");
});

// 内核能力初始化：与界面渲染并行，逐个失败互不影响。
//
// 「就绪」的定义：**i18n 与主题已就位**——这两者决定首屏文案与配色，
// 未就绪就显示主界面会出现文案回退键名、配色闪白。平台形态同样在首屏前
// 确定（决定 Shell 取向是桌面还是移动），故一并纳入就绪等待。
// 其余能力（设置 / 下载 / 账户）在后台继续初始化，由各自的事件驱动，不阻塞首屏。
//
// 无论成功失败都要置就绪：内核不可用（浏览器调试 / 后端异常）时不能把用户
// 永远留在加载页，主界面自身的错误处理会给出反馈。
void Promise.all([
  initI18n(),
  initTheme(),
  initPlatform(),
  initSettings(),
  initDownloads(),
  initAccount(),
]).finally(() => {
  markKernelReady();
});
