// 开始页模块前端注册点。
//
// 由 `../index.ts` 引入，把开始页的左导航入口与内容区路由登记到内核模块注册表
// （`../registry.ts`）。路由：
// - `/`：简洁模式首页（接管内核原占位首页）；
// - `/version-settings`：实例入口页（封面 / 信息 / 模式分段 / 动作）；
// - `/version-settings/content`：实例资源页；
// - `/version-settings/mods`：实例模组页；
// - `/version-settings/client`：实例客户端页（DLL 与启动状态）；
// - `/version-settings/controls`：实例屏幕触控层页（仅移动端入口）。
//
// 子页面共用 `?name=<实例名>`；面包屑都指回 `/version-settings`，实例选择由
// `useVersionInstance` 的模块级记忆兜住，跳回入口页不会丢上下文。

import { House } from "@lucide/vue";

import { registerModule } from "../registry";

/** 面包屑前两段：开始 / 版本设置。子页面在末尾追加自己的段落。 */
const SETTINGS_CRUMBS = [
  { titleKey: "module.home.title", path: "/" },
  { titleKey: "module.home.settings", path: "/version-settings" },
];

registerModule({
  id: "home",
  nav: {
    id: "home",
    path: "/",
    titleKey: "module.home.title",
    icon: House,
  },
  routes: [
    {
      path: "/",
      name: "home",
      component: () => import("./HomePage.vue"),
      meta: { titleKey: "module.home.title" },
    },
    {
      path: "/version-settings",
      name: "version-settings",
      component: () => import("./InstanceEntry.vue"),
      meta: {
        titleKey: "module.home.settings",
        // 标题栏以导航栏样式的面包屑呈现「开始 / 版本设置」，前段可点击返回。
        breadcrumb: SETTINGS_CRUMBS,
      },
    },
    {
      path: "/version-settings/content",
      name: "version-settings-content",
      component: () => import("./InstanceContentPage.vue"),
      meta: {
        titleKey: "module.home.instance.page.content",
        breadcrumb: [...SETTINGS_CRUMBS, { titleKey: "module.home.instance.page.content" }],
      },
    },
    {
      path: "/version-settings/mods",
      name: "version-settings-mods",
      component: () => import("./InstanceModsPage.vue"),
      meta: {
        titleKey: "module.home.instance.page.mods",
        breadcrumb: [...SETTINGS_CRUMBS, { titleKey: "module.home.instance.page.mods" }],
      },
    },
    {
      path: "/version-settings/client",
      name: "version-settings-client",
      component: () => import("./InstanceClientPage.vue"),
      meta: {
        titleKey: "module.home.instance.page.client",
        breadcrumb: [...SETTINGS_CRUMBS, { titleKey: "module.home.instance.page.client" }],
      },
    },
    {
      path: "/version-settings/controls",
      name: "version-settings-controls",
      component: () => import("./InstanceControlsPage.vue"),
      meta: {
        titleKey: "module.home.instance.page.controls",
        breadcrumb: [...SETTINGS_CRUMBS, { titleKey: "module.home.instance.page.controls" }],
      },
    },
  ],
});
