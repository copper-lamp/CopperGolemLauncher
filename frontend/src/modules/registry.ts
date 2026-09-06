// 内核模块前端注册扩展点：内置 / 附加模块在此登记自己的左导航入口与内容区路由。
//
// 动机：内核阶段路由与左导航是静态写死的；进入模块阶段后各内置模块并行开发，
// 若都去改 `router/index.ts` 与 `SideNav.vue` 会互相冲突。本注册表让每个模块
// 独立声明「侧边栏入口（单 icon）+ 内容区子路由」，由内核统一收集并注入导航与路由。
//
// 约定（对齐架构总览「模块前端包注册机制」）：
// - 每个模块调用一次 `registerModule({ id, nav, routes })`。
// - `nav.path` 应为该模块列表页路由；`titleKey` 走 i18n（模块命名空间 `module.<id>.*`）。
// - `routes` 为 Vue Router 记录，组件允许懒加载（推荐 `() => import(...)`）。
// - 模块之间不互相依赖，仅经本注册表与内核 Shell 交互。

import type { Component } from "vue";
import type { RouteRecordRaw } from "vue-router";

/** 侧边栏入口（单 icon 无文字）。 */
export interface ModuleNavItem {
  /** 所属模块 id。 */
  id: string;
  /** 点击跳转的列表页路径。 */
  path: string;
  /** 左导航 title / tooltip 的 i18n 键。 */
  titleKey: string;
  /** 导航图标组件（来自 @lucide/vue）。 */
  icon: Component;
}

/** 一个前端模块包。 */
export interface ModuleFrontend {
  id: string;
  nav: ModuleNavItem;
  routes: RouteRecordRaw[];
}

const modules = new Map<string, ModuleFrontend>();
const navOrder: string[] = [];

/** 注册一个前端模块。重复 id 抛出，避免静默覆盖导致行为不可预期。 */
export function registerModule(module: ModuleFrontend): void {
  if (modules.has(module.id)) {
    throw new Error(`[modules] 前端模块 \`${module.id}\` 已注册`);
  }
  modules.set(module.id, module);
  navOrder.push(module.id);
}

/** 已注册的全部模块（按注册顺序）。 */
export function getModuleFrontends(): ModuleFrontend[] {
  return navOrder.map((id) => modules.get(id)!).filter(Boolean);
}

/** 侧边栏导航项（按注册顺序）。 */
export function getModuleNav(): ModuleNavItem[] {
  return getModuleFrontends().map((m) => m.nav);
}

/** 模块内容区路由。 */
export function getModuleRoutes(): RouteRecordRaw[] {
  return getModuleFrontends().flatMap((m) => m.routes);
}