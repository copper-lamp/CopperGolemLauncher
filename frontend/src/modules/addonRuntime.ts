// 附加模块前端运行时：取入口清单、注入宿主桥、动态加载 ESM 并登记导航与路由。
//
// # 安全边界
//
// 模块代码在宿主 WebView 内执行，这是规格认可的**信任边界**——安装即信任代码。
// 但信任不等于放任，宿主只暴露一个窄化桥：
// - 桥不提供内核注册表或任意命令；同一 WebView 的可信代码不具备安全隔离；
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

import { watch, type Component } from "vue";
import { listen } from "@tauri-apps/api/event";
import { registerAddonI18n, translateAddon, useLocaleRef } from "../i18n";
import type { RouteRecordRaw, Router } from "vue-router";

import { invoke } from "@tauri-apps/api/core";

import { registerModule, unregisterModule } from "./registry";
import { registerSettings, disposeModuleSettings, type SettingsContribution } from '../services/settings-registry';

/** 内核 `modules_frontends` 返回的一项。 */
export interface AddonFrontendDescriptor {
  id: string;
  namespace: string;
  entry_url: string;
  style_urls: string[];
  /** From real manifest subscribe/publish declarations, dot-to-hyphen mapped. */
  frontend_events?: string[];
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
  registerSettings(section: SettingsContribution): Promise<() => void>;
  /** 调用**本模块自身**的命令。 */
  invoke(command: string, args?: unknown): Promise<unknown>;
  registerI18n(packs: Record<string, Record<string, string>>): void;
  t(key: string, params?: Record<string, string | number>): string;
  locale(): string;
  onLocaleChange(handler: (locale: string) => void): () => void;
  listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<() => void>;
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

/** Fetch descriptors; propagate failures to boot diagnostics rather than reporting false success. */
export async function fetchAddonFrontends(): Promise<AddonFrontendDescriptor[]> {
  return await invoke<AddonFrontendDescriptor[]>("modules_frontends");
}

/**
 * 加载全部附加模块前端，并把它们登记进左导航与路由。
 *
 * 核心先挂载；导航注册表响应式更新，路由先登记再发布导航。
 */
let runtime: Promise<AddonFrontendLoadReport[]> | undefined;
export function loadAddonFrontends(router: Router): Promise<AddonFrontendLoadReport[]> {
  return runtime ??= startAddonRuntime(router);
}
async function startAddonRuntime(router: Router): Promise<AddonFrontendLoadReport[]> {
  const loaded = new Map<string, () => void>();
  const pending = new Map<string, { descriptor: AddonFrontendDescriptor; dispose: () => void }>();
  const attempted = new Set<string>();
  const activations = new Map<string, number>();
  const liveDescriptors = new Map<string, AddonFrontendDescriptor>();
  let desired = new Map<string, AddonFrontendDescriptor>();
  let revision = 0;
  let worker: Promise<AddonFrontendLoadReport[]> | undefined;
  let snapshotSeq = 0;
  const cleanups = (id: string, disposers: (() => void)[]) => {
    for (const dispose of disposers.splice(0).reverse()) { try { dispose(); } catch (error) { console.warn('[addons] cleanup failed', error); } }
    disposeModuleSettings(id);
    void invoke('settings_module_unregister', {moduleId:id,sectionId:'*'}).catch(error => console.warn('[addons] settings cleanup failed', error));
  };
  const drain = async (): Promise<AddonFrontendLoadReport[]> => {
    const reports: AddonFrontendLoadReport[] = [];
    let observed = -1;
    while (observed !== revision) {
      observed = revision;
      for (const descriptor of desired.values()) {
        if (attempted.has(descriptor.id)) continue;
        attempted.add(descriptor.id);
        const rollback: (() => void)[] = [];
        let active = true;
        const dispose = () => { active = false; cleanups(descriptor.id, rollback); };
        pending.set(descriptor.id, { descriptor, dispose });
        try {
          rollback.push(() => { for (const link of document.querySelectorAll<HTMLLinkElement>('link[data-addon-styles]')) if (link.dataset.addonStyles === descriptor.id) link.remove(); });
          injectStyles(descriptor);
          installHostBridge(descriptor, router, rollback, () => active && desired.get(descriptor.id) === descriptor);
          const activation = (activations.get(descriptor.id) ?? 0) + 1;
          activations.set(descriptor.id, activation);
          const entry = new URL(descriptor.entry_url, window.location.href);
          if (activation > 1) entry.searchParams.set("copper_activation", String(activation));
          await import(/* @vite-ignore */ entry.href);
          if (desired.get(descriptor.id) !== descriptor) { dispose(); continue; }
          loaded.set(descriptor.id, dispose);
          liveDescriptors.set(descriptor.id, descriptor);
          reports.push({ id: descriptor.id, loaded: true });
        } catch (error) {
          dispose();
          const message = error instanceof Error ? error.message : String(error);
          console.warn('[addons] frontend failed', descriptor.id, message);
          reports.push({ id: descriptor.id, loaded: false, error: message });
        } finally { pending.delete(descriptor.id); window.__COPPER_HOST__ = undefined; }
      }
    }
    const current = router.currentRoute.value;
    if (current.matched.some(record => record.name && !router.hasRoute(record.name))) await router.replace("/");
    else if (!current.matched.length && router.resolve(current.fullPath).matched.length) await router.replace(current.fullPath);
    return reports;
  };
  const refresh = async (): Promise<AddonFrontendLoadReport[]> => {
    const sequence = ++snapshotSeq;
    const descriptors = await fetchAddonFrontends();
    if (sequence !== snapshotSeq) return [];
    const next = new Map(descriptors.map(descriptor => [descriptor.id, descriptor]));
    // Preserve identity while the same installed entry remains live.
    for (const [id, descriptor] of next) {
      const previous = desired.get(id);
      if (previous && JSON.stringify(previous) === JSON.stringify(descriptor)) next.set(id, previous);
    }
    desired = next;
    revision++;
    for (const [id, entry] of pending) if (desired.get(id) !== entry.descriptor) { entry.dispose(); attempted.delete(id); }
    for (const [id, dispose] of loaded) if (desired.get(id) !== liveDescriptors.get(id)) { dispose(); loaded.delete(id); liveDescriptors.delete(id); attempted.delete(id); }
    for (const id of attempted) if (!desired.has(id)) attempted.delete(id);
    if (!worker) worker = drain().finally(() => { worker = undefined; });
    return worker;
  };
  const changed = () => { void refresh().then(reports => { for (const report of reports) if (!report.loaded) console.warn('[addons]', report.id, report.error); }).catch(error => console.warn('[addons] refresh failed', error)); };
  // Subscribe before snapshot: a boot-complete event cannot fall into a missing-snapshot gap.
  // Late subscription completion takes a new snapshot as well, closing the timeout gap.
  // Broken subscriptions are reported and bounded; they never gate the core UI.
  let timer: ReturnType<typeof setTimeout> | undefined;
  await Promise.race([
    Promise.allSettled([
      listen('modules-changed', changed).then(stop => { changed(); return stop; }),
      listen<Record<string, unknown>>('settings-changed', event => { if (Object.keys(event.payload).some(key => key.startsWith('modules.') && key.endsWith('.enabled'))) changed(); }),
    ]).then(results => { for (const result of results) if (result.status === 'rejected') console.warn('[addons] observer failed', result.reason); }),
    new Promise<void>(resolve => { timer = setTimeout(resolve, 1000); }),
  ]);
  if (timer) clearTimeout(timer);
  return refresh();
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

function installHostBridge(descriptor: AddonFrontendDescriptor, router: Router, rollback: (() => void)[], isActive: () => boolean): void {
  const assertActive = () => { if (!isActive()) throw new Error("Addon is no longer active"); };
  window.__COPPER_HOST__ = {
    async registerSettings(section) {
      assertActive();
      await invoke('settings_module_register',{moduleId:descriptor.id,section});
      try {
        assertActive();
        const dispose=registerSettings({id:descriptor.id,namespace:descriptor.namespace,t:(key)=>translateAddon(descriptor.namespace,key),read:(key)=>invoke('settings_module_get',{moduleId:descriptor.id,key}),write:(key,value)=>invoke('settings_module_set',{moduleId:descriptor.id,key,value})},section);
        return ()=>{dispose();void invoke('settings_module_unregister',{moduleId:descriptor.id,sectionId:section.id}).catch(()=>{});};
      }catch(error){void invoke('settings_module_unregister',{moduleId:descriptor.id,sectionId:section.id}).catch(()=>{});throw error;}
    },
    registerI18n(packs) {
      assertActive(); registerAddonI18n(descriptor.namespace, packs); },
    t(key, params) { return translateAddon(descriptor.namespace, key, params); },
    locale() { return useLocaleRef().value; },
    onLocaleChange(handler) {
      assertActive(); const stop = watch(useLocaleRef(), (value) => handler(value)); rollback.push(stop); return stop; },
    listen<T>(event: string, handler: (event: { payload: T }) => void) {
      assertActive();
      // Exact declarations and suffix wildcards only, matching the kernel manifest rules.
      const allowed = descriptor.frontend_events?.some((pattern) => pattern.endsWith("*")
        ? event.startsWith(pattern.slice(0, -1)) : pattern === event);
      if (!allowed || !/^[a-zA-Z0-9_:/-]+$/.test(event)) return Promise.reject(new Error("Event not declared by this module"));
      return listen<T>(event, handler).then((stop) => { if (!isActive()) { stop(); throw new Error("Addon is no longer active"); } rollback.push(stop); return stop; });
    },
    registerModule(registration) {
      assertActive();
      // Never overwrite an existing named route, including builtins.
      const check = (routes: RouteRecordRaw[]): void => { for (const route of routes) { if (route.name && router.hasRoute(route.name)) throw new Error("Route name already registered: " + String(route.name)); if (route.children) check(route.children); } };
      check(registration.routes);
      for (const route of registration.routes) rollback.push(router.addRoute(route));
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
      rollback.push(() => unregisterModule(descriptor.id));
    },
    invoke(command, args) {
      assertActive();
      return invoke("module_invoke", {
        moduleId: descriptor.id,
        command,
        args: args ?? null,
      });
    },
  };
}
