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
