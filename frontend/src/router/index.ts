// 应用路由：内核提供 Downloads / Settings；
// `/`（首页）与各模块入口由内置模块前端包（`frontend/src/modules/*/register.ts`）
// 经模块注册表登记，路由按注册顺序注入。开始页模块接管 `/` 与 `/version-settings`。

import { createRouter, createWebHashHistory } from "vue-router";

// 先触发各模块注册，再收集模块路由（见 main 装配时序）。
import "../modules";
import { getModuleRoutes } from "../modules/registry";

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
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
      // 储存有独立的二级页：一级页只显示用量与入口，清理在 /settings/storage/cleanup。
      // 其余分类仍是一级页里的面板，靠 query.tab 记忆，不占用路由。
      children: [
        {
          path: "storage",
          name: "settings-storage",
          component: () => import("../views/settings/StorageTab.vue"),
          meta: { titleKey: "settings.tabs.storage" },
        },
        {
          path: "storage/cleanup",
          name: "settings-storage-cleanup",
          component: () => import("../views/settings/StorageCleanupPage.vue"),
          meta: { titleKey: "settings.storage.cleanup.title" },
        },
      ],
    },
  ],
});

// 注入各模块登记的内容区路由（含首页 `/` 与开始页子路由）。
for (const route of getModuleRoutes()) {
  router.addRoute(route);
}

export default router;
