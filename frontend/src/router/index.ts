// 应用路由：内核阶段提供 Home / Downloads / Settings；
// 模块开发阶段由各模块注册其入口路由与左导航项。

import { createRouter, createWebHashHistory } from "vue-router";

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    {
      path: "/",
      name: "home",
      component: () => import("../views/HomeView.vue"),
      meta: { titleKey: "app.name" },
    },
    {
      path: "/downloads",
      name: "downloads",
      component: () => import("../views/DownloadsView.vue"),
      meta: { titleKey: "download.title" },
    },
    {
      path: "/settings",
      name: "settings",
      component: () => import("../views/SettingsView.vue"),
      meta: { titleKey: "settings.title" },
    },
  ],
});

export default router;
