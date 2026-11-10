// Shell owns visit trails; modules only need their existing route registration.
import { computed, ref } from "vue";
import type { RouteLocationNormalizedLoaded, Router } from "vue-router";
import { getModuleFrontends } from "../modules/registry";

export interface NavigationVisit { fullPath: string; titleKey?: string; title?: string; fallback: string }
const visits = ref<NavigationVisit[]>([]);
const canBack = ref(false);
const canForward = ref(false);
let initialized = false;

export function navigationScopeOf(route: RouteLocationNormalizedLoaded): string {
  for (const module of getModuleFrontends()) {
    const owns = (records: typeof module.routes): boolean => records.some(record =>
      route.matched.some(match => record.name != null ? match.name === record.name : match.path === record.path)
      || (record.children ? owns(record.children) : false));
    if (owns(module.routes)) return `module:${module.id}`;
  }
  return `page:${route.matched[0]?.path ?? route.path}`;
}
function visitOf(route: RouteLocationNormalizedLoaded): NavigationVisit {
  const raw = route.meta.breadcrumb;
  const last = Array.isArray(raw) ? raw[raw.length - 1] as { title?: unknown } | undefined : undefined;
  return { fullPath: route.fullPath, titleKey: typeof route.meta.titleKey === "string" ? route.meta.titleKey : undefined,
    title: typeof last?.title === "string" && last.title ? last.title : undefined,
    fallback: typeof route.name === "string" ? route.name : route.path };
}
export function initNavigationTrail(router: Router): void {
  if (initialized) return;
  initialized = true;
  let scope = "";
  let previousPosition: number | undefined;
  let previousForward: unknown;
  const snapshots = new Map<number, NavigationVisit[]>();
  router.afterEach((to, _from, failure) => {
    if (failure) return;
    // Dynamic titles can arrive after navigation (content API, locale updates).
    if (visits.value.length && visits.value[visits.value.length - 1].fullPath === _from.fullPath) {
      visits.value = [...visits.value.slice(0, -1), visitOf(_from)];
      if (previousPosition !== undefined) snapshots.set(previousPosition, [...visits.value]);
    }
    const state = router.options.history.state;
    const position = typeof state.position === "number" ? state.position : 0;
    const nextScope = navigationScopeOf(to);
    const visit = visitOf(to);
    const popped = previousPosition !== undefined && (position < previousPosition || (position > previousPosition && previousForward === to.fullPath));
    if (!popped && previousPosition !== undefined && position > previousPosition) {
      for (const key of snapshots.keys()) if (key >= position) snapshots.delete(key);
    }
    if (nextScope !== scope) visits.value = [visit];
    else if (popped && snapshots.has(position)) {
      visits.value = [...snapshots.get(position)!];
    } else {
      const existing = visits.value.findIndex(item => item.fullPath === to.fullPath);
      if (existing >= 0) visits.value = [...visits.value.slice(0, existing), visit];
      else if (position === previousPosition) visits.value = [...visits.value.slice(0, -1), visit];
      else visits.value = [...visits.value, visit].slice(-32);
    }
    // Never expose stale routes after a module has removed them.
    visits.value = visits.value.filter(item => router.resolve(item.fullPath).matched.length > 0);
    scope = nextScope;
    previousPosition = position;
    previousForward = state.forward;
    snapshots.set(position, [...visits.value]);
    if (snapshots.size > 128) snapshots.delete(snapshots.keys().next().value!);
    canBack.value = typeof state.back === "string" && router.resolve(state.back).matched.length > 0;
    canForward.value = typeof state.forward === "string" && router.resolve(state.forward).matched.length > 0;
  });
}
export function useNavigationTrail() {
  return { visits: computed(() => visits.value), canBack: computed(() => canBack.value), canForward: computed(() => canForward.value) };
}
