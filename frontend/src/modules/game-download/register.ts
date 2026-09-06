// 游戏下载模块前端注册点。
//
// 由 `../index.ts` 引入，把游戏下载模块的左导航入口与内容区路由登记到
// 内核模块注册表（`../registry.ts`），由内核 Shell 统一注入 SideNav 与 Router。
//
// 路由：
// - `/game-download`：版本清单页（最新正式/预览 + 按大版本分组 + 下载入口）；
// - `/game-download/:id`：单版本详情页（任务状态 / 进度 / 取消）。
//
// 语言包走 `module.game-download.*` 命名空间（与后端 `register_module_pack`
// 及前端 `i18n/index.ts` 的模块兜底一致）。

import { Gamepad2 } from "@lucide/vue";

import { registerModule } from "../registry";

registerModule({
  id: "game-download",
  nav: {
    id: "game-download",
    path: "/game-download",
    titleKey: "module.game-download.navTitle",
    icon: Gamepad2,
  },
  routes: [
    {
      path: "/game-download",
      name: "game-download-list",
      component: () => import("./GameDownloadPage.vue"),
      meta: { titleKey: "module.game-download.listTitle" },
    },
    {
      path: "/game-download/:id",
      name: "game-download-detail",
      component: () => import("./VersionDetailPage.vue"),
      meta: { titleKey: "module.game-download.detailTitle" },
    },
  ],
});