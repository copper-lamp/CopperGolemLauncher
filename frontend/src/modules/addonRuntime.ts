// 附加模块前端运行时：取入口清单、注入宿主桥、动态加载 ESM 并登记导航与路由。
//
// # 安全边界
//
// 模块代码在宿主 WebView 内执行，这是规格认可的**信任边界**——安装即信任代码。
// 但信任不等于放任，宿主只暴露一个窄化桥：
// - 模块拿不到内核注册表，也拿不到 Tauri 命令表；
// - `invoke` 只允许调用**本模块自己**的命令：模块 id 由宿主绑定，不由模块声明。
//
// # 错误隔离
//
// 单个模块加载失败只记录并跳过，既不阻断内置界面，也不影响其它模块。这一点很关键：
// 一个坏模块不该让用户整个启动器不可用。
//
// # 为什么需要 vue / vue-router
//
// 模块注册的是 Vue 组件与路由记录，必须与宿主**共用同一个 Vue 实例**：各带一份
// Vue 会让组件无法互操作。因此模板把 `vue` / `vue-router` 标记为外部依赖，由
// `index.html` 的 import map 指向宿主提供的 vendor 文件（见 scripts/copy-vendor.mjs）。

import type { Component } from "vue";
import type { RouteRecordRaw, Router } from "vue-router";

import { invoke } from "@tauri-apps/api/core";

import { registerModule } from "./registry";

/** 内核 `modules_frontends` 返回的一项。 */
export interface AddonFrontendDescriptor {
  id: string;
  namespace: string;
  entry_url: string;
  style_urls: string[];
}

/** 模块提交的登记内容（模块 id 由宿主绑定，模块无权声明）。 */
export interface AddonModuleRegistration {
  nav: {
    path: string;
    titleKey: string;
    icon: Component;
  };
  routes: RouteRecordRaw[];
}

/** 暴露给模块脚本的宿主桥。 */
export interface CopperHostBridge {
  /** 登记本模块的左导航入口与内容区路由。 */
  registerModule(registration: AddonModuleRegistration): void;
  /** 调用**本模块自身**的命令。 */
  invoke(command: string, args?: unknown): Promise<unknown>;
}

declare global {
  interface Window {
    __COPPER_HOST__?: CopperHostBridge;
  }
}

/** 单个模块的加载结果，供启动日志与自检使用。 */
export interface AddonFrontendLoadReport {
  id: string;
  loaded: boolean;
  error?: string;
}

/** 拉取可用的附加模块前端清单；内核不可用时返回空表而不抛出。 */
export async function fetchAddonFrontends(): Promise<AddonFrontendDescriptor[]> {
  try {
    return await invoke<AddonFrontendDescriptor[]>("modules_frontends");
  } catch (error) {
    console.warn("[addons] 无法获取附加模块前端清单：", error);
    return [];
  }
}

/**
 * 加载全部附加模块前端，并把它们登记进左导航与路由。
 *
 * 必须在应用挂载**之前**完成：左导航与路由表在首次渲染时被读取，之后追加路由会
 * 出现"导航项存在但首次点击 404"的竞态。
 */
export async function loadAddonFrontends(
  router: Router,
): Promise<AddonFrontendLoadReport[]> {
  const descriptors = await fetchAddonFrontends();
  const reports: AddonFrontendLoadReport[] = [];

  for (const descriptor of descriptors) {
    try {
      injectStyles(descriptor);
      installHostBridge(descriptor, router);
      await import(/* @vite-ignore */ descriptor.entry_url);
      reports.push({ id: descriptor.id, loaded: true });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      console.warn(`[addons] 附加模块 \`${descriptor.id}\` 的前端加载失败：${message}`);
      reports.push({ id: descriptor.id, loaded: false, error: message });
    } finally {
      // 桥一次只服务一个模块。留在全局会让后加载的脚本冒充前一个模块的身份。
      window.__COPPER_HOST__ = undefined;
    }
  }

  return reports;
}

function injectStyles(descriptor: AddonFrontendDescriptor): void {
  for (const url of descriptor.style_urls) {
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = url;
    link.dataset.addonStyles = descriptor.id;
    document.head.appendChild(link);
  }
}

function installHostBridge(descriptor: AddonFrontendDescriptor, router: Router): void {
  window.__COPPER_HOST__ = {
    registerModule(registration) {
      registerModule({
        id: descriptor.id,
        nav: {
          id: descriptor.id,
          path: registration.nav.path,
          titleKey: registration.nav.titleKey,
          icon: registration.nav.icon,
        },
        routes: registration.routes,
      });
      for (const route of registration.routes) {
        router.addRoute(route);
      }
    },
    invoke(command, args) {
      return invoke("module_invoke", {
        moduleId: descriptor.id,
        command,
        args: args ?? null,
      });
    },
  };
}
