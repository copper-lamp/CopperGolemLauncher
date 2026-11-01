// 内容下载模块前端注册点。
//
// 由 `../index.ts` 引入，把内容下载模块的左导航入口与内容区路由登记到
// 内核模块注册表（`../registry.ts`），由内核 Shell 统一注入 SideNav 与 Router。
//
// 路由：
// - `/content`：内容列表页（搜索 / 过滤 / 分页 / 卡片徽标）；
// - `/content/:id`：内容详情页（readme / 版本分类 / 依赖 / 下载弹窗 + 抛物线动画）。
//
// 语言包走 `module.content-download.*` 命名空间（与后端 `register_module_pack`
// 及前端 `i18n/index.ts` 的模块兜底一致）。

import { PackageOpen } from "@lucide/vue";

import { registerModule } from "../registry";
import { detailCrumbTitle } from "./detailCrumb";

registerModule({
  id: "content-download",
  nav: {
    id: "content-download",
    path: "/content",
    titleKey: "module.content-download.navTitle",
    icon: PackageOpen,
  },
  routes: [
    {
      path: "/content",
      name: "content-list",
      component: () => import("./ListView.vue"),
      meta: {
        titleKey: "module.content-download.listTitle",
        breadcrumb: [{ titleKey: "module.content-download.listTitle", path: "/content" }],
      },
    },
    {
      path: "/content/:id",
      name: "content-detail",
      component: () => import("./DetailView.vue"),
      meta: {
        titleKey: "module.content-download.detailTitle",
        backPath: "/content",
        // 末级标题是当前内容名，由 `detailCrumbTitle` 这个响应式引用提供：
        // 直接改 meta 里的字面量会把上一页的名字留在共享的数组里（切详情时先看到旧名字）。
        breadcrumb: [
          { titleKey: "module.content-download.listTitle", path: "/content" },
          {
            titleKey: "module.content-download.detailTitle",
            path: "",
            get title() {
              return detailCrumbTitle.value;
            },
          },
        ],
      },
    },
  ],
});