// 开始页模块前端注册点。
//
// 由 `../index.ts` 引入，把开始页的左导航入口与内容区路由登记到内核模块注册表
// （`../registry.ts`）。路由：
// - `/`：简洁模式首页（接管内核原占位首页）；
// - `/version-settings`：版本设置页（左右布局，左侧版本 Tabs）。
//
// 默认模式（Win10 磁贴风格）本次不实现，保留模式切换占位（见 HomePage.vue）。

import { House } from "@lucide/vue";

import { registerModule } from "../registry";

registerModule({
  id: "home",
  nav: {
    id: "home",
    path: "/",
    titleKey: "home.title",
    icon: House,
  },
  routes: [
    {
      path: "/",
      name: "home",
      component: () => import("./HomePage.vue"),
      meta: { titleKey: "home.title" },
    },
    {
      path: "/version-settings",
      name: "version-settings",
      component: () => import("./VersionSettings.vue"),
      meta: { titleKey: "home.settings" },
    },
  ],
});
