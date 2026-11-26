<script setup lang="ts">
// Shell history controls are global. Breadcrumbs show only visits inside the current module.
// Existing route metadata supplies localized/dynamic labels; pages inject actions via Teleport.

import { ref, computed, onMounted, onUnmounted } from "vue";
import { Minus, Square, Copy, X, ArrowLeft, ArrowRight, Search, Bell, ChevronRight, Stethoscope, FolderOpen, Download, Languages, Trash2 } from "@lucide/vue";
import { useNavigationTrail } from "../composables/useNavigationTrail";
import { showToast } from "../composables/useToast";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useRoute, useRouter } from "vue-router";

import AccountMenu from "./AccountMenu.vue";
import AndroidAccountCard from "./AndroidAccountCard.vue";
import UpdateBadge from "./update/UpdateBadge.vue";
import { setDiagnosticOpen } from "../diag";
import { useBootDiagnostics } from "../boot";
import { useI18n } from "../i18n";
import { usePlatform } from "../composables/usePlatform";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();

// 移动端没有窗口装饰，最小化 / 最大化 / 关闭按钮无意义，需隐藏（见 docs/平台适配.md 2.5）。
const { isMobile, isAndroid } = usePlatform();

/**
 * 启动异常入口。
 *
 * 只在引导确实出过问题时出现：它是给用户的**唯一**在手机上打开诊断面板的途径——
 * 移动端没有地址栏，`?diag=1` 这条路对真实用户等于不存在。
 */
const { steps: bootSteps } = useBootDiagnostics();
const hasBootTrouble = computed(() =>
  bootSteps.value.some((step) => step.state === "failed" || step.state === "timeout"),
);

/** 当前路由标题（i18n）与返回目标路径。 */
const title = computed(() => {
  const key = route.meta.titleKey;
  return typeof key === "string" ? t(key) : "";
});
const { visits, canBack, canForward } = useNavigationTrail();

/** 面包屑单段。 */
interface BreadcrumbItem {
  /** 段标题的 i18n 键（与 `title` 二选一，优先级低于 `title`）。 */
  titleKey?: string;
  /** 段标题字面量：用于动态名称（如具体内容名），无需预先声明 i18n 键。 */
  title?: string;
  /** 点击跳转路径（末段不跳转）。 */
  path: string;
}

/** 路由声明的面包屑（非数组 / 结构不合法时视为未声明）。 */
const breadcrumb = computed<BreadcrumbItem[]>(() => {
  const raw = route.meta.breadcrumb;
  const declaredLast = Array.isArray(raw) ? raw[raw.length - 1] as Partial<BreadcrumbItem> | undefined : undefined;
  return visits.value.map((visit, index) => ({
    path: visit.fullPath,
    titleKey: visit.titleKey,
    title: index === visits.value.length - 1 && typeof declaredLast?.title === "string" && declaredLast.title
      ? declaredLast.title : visit.title || (visit.titleKey ? undefined : visit.fallback),
  }));
});

/** 段标题文本：字面量优先，其次 i18n 键。 */
function crumbText(item: BreadcrumbItem): string {
  if (typeof item.title === "string" && item.title.length > 0) return item.title;
  return typeof item.titleKey === "string" ? t(item.titleKey) : "";
}

/** 面包屑跳转（保留完整路径语义，直接 push）。 */
function goCrumb(path: string) {
  if (!path) return;
  void router.push(path);
}

/** 有可退历史时优先 router.back()（保留内容列表滚动位置），否则回退到 backPath。 */
function goBack() {
  if (canBack.value) router.back();
}

interface SearchItem { id: "translation" | "download" | "storage" | "storage-cleanup"; icon: typeof Languages; terms: string[]; }
const searchOpen = ref(false);
const searchQuery = ref("");
const searchItems: SearchItem[] = [
  { id: "translation", icon: Languages, terms: ["翻译", "语言", "translation", "language"] },
  { id: "download", icon: Download, terms: ["下载", "下载设置", "download", "downloads"] },
  { id: "storage", icon: FolderOpen, terms: ["储存", "存储", "储存位置", "存储位置", "storage", "location"] },
  { id: "storage-cleanup", icon: Trash2, terms: ["清理", "清理储存", "clean", "cleanup", "cache"] },
];
const searchResults = computed(() => {
  const query = searchQuery.value.trim().toLocaleLowerCase();
  return searchItems
    .map((item) => ({
      ...item,
      label: item.id === "storage-cleanup" ? t("settings.storage.cleanup.title") : t(`settings.tabs.${item.id}`),
    }))
    .filter((item) => !query || [item.label, ...item.terms].some((term) => term.toLocaleLowerCase().includes(query)))
    .slice(0, 5);
});
function toggleSearch() {
  searchOpen.value = !searchOpen.value;
  if (!searchOpen.value) searchQuery.value = "";
}
function closeSearch() {
  searchOpen.value = false;
  searchQuery.value = "";
}
function goSearchResult(id: SearchItem["id"]) {
  closeSearch();
  if (id === "storage" || id === "storage-cleanup") {
    void router.push({ name: id === "storage" ? "settings-storage" : "settings-storage-cleanup" });
    return;
  }
  void router.push({ path: "/settings", query: { tab: id } });
}
function onSearchKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") closeSearch();
}
function onDocumentPointerdown(event: PointerEvent) {
  if (!searchOpen.value) return;
  const target = event.target;
  if (target instanceof Element && !target.closest(".titlebar__search")) closeSearch();
}

const maximized = ref(false);
let unlistenMaximize: (() => void) | null = null;

const appWindow = getCurrentWindow();

/** Bubble from blank header surfaces only; controls and scrollable breadcrumbs own their gestures. */
function startWindowDrag(event: PointerEvent) {
  if (isMobile.value || !event.isPrimary || event.button !== 0) return;
  const target = event.target;
  if (!(target instanceof Element) || target.closest('button, a, input, select, textarea, [role="button"], [contenteditable], .titlebar__crumbs, .titlebar__actions, .titlebar__right')) return;
  event.preventDefault();
  void appWindow.startDragging().catch(error => showToast(String(error), 'error'));
}

onMounted(async () => {
  document.addEventListener("pointerdown", onDocumentPointerdown);
  document.addEventListener("keydown", onSearchKeydown);
  try {
    maximized.value = await appWindow.isMaximized();
    unlistenMaximize = await appWindow.onResized(() => {
      void appWindow.isMaximized().then((m) => (maximized.value = m));
    });
  } catch {
    // 非桌面环境（如浏览器调试）忽略。
  }
});

onUnmounted(() => {
  document.removeEventListener("pointerdown", onDocumentPointerdown);
  document.removeEventListener("keydown", onSearchKeydown);
  unlistenMaximize?.();
});

function minimize() {
  void appWindow.minimize();
}

function toggleMaximize() {
  void appWindow.toggleMaximize();
}

function close() {
  void appWindow.close();
}
</script>

<template>
  <header class="titlebar" @pointerdown="startWindowDrag">
    <div class="titlebar__left">
      <button class="titlebar__back" :disabled="!canBack" :title="t('titlebar.back')" :aria-label="t('titlebar.back')" @click="goBack"><ArrowLeft :size="18" /></button>
      <button class="titlebar__back" :disabled="!canForward" :title="t('titlebar.forward')" :aria-label="t('titlebar.forward')" @click="router.forward()"><ArrowRight :size="18" /></button>
      <nav v-if="breadcrumb.length" class="titlebar__crumbs">
        <template v-for="(crumb, index) in breadcrumb" :key="`${index}-${crumb.path}`">
          <button
            v-if="index < breadcrumb.length - 1"
            class="titlebar__crumb"
            @click="goCrumb(crumb.path)"
          >
            {{ crumbText(crumb) }}
          </button>
          <span v-else class="titlebar__crumb titlebar__crumb--current" aria-current="page">
            {{ crumbText(crumb) }}
          </span>
          <ChevronRight
            v-if="index < breadcrumb.length - 1"
            class="titlebar__crumb-sep"
            :size="12"
          />
        </template>
      </nav>
      <template v-else>
        <h1 v-if="title" class="titlebar__title">{{ title }}</h1>
      </template>
    </div>
    <div class="titlebar__spacer" aria-hidden="true" />
    <!-- 更新入口：位于操作按钮左侧，独立控件，不参与操作区的 flex 布局 -->
    <UpdateBadge />
    <!-- 页面注入操作区：各模块经 <Teleport to="#copper-titlebar-actions"> 放置按钮/切换。 -->
    <div class="titlebar__actions" id="copper-titlebar-actions" />
    <div class="titlebar__right">
      <button
        v-if="hasBootTrouble"
        class="titlebar__trouble"
        type="button"
        :title="t('boot.diag.title')"
        :aria-label="t('boot.diag.title')"
        @click="setDiagnosticOpen(true)"
      >
        <Stethoscope :size="15" />
      </button>
      <div class="titlebar__search">
        <button class="titlebar__back" :title="t('titlebar.search')" :aria-label="t('titlebar.search')" :aria-expanded="searchOpen" @click="toggleSearch"><Search :size="18" /></button>
        <section v-if="searchOpen" class="titlebar__search-popover" role="dialog" :aria-label="t('titlebar.search')">
          <label class="titlebar__search-field">
            <Search :size="16" aria-hidden="true" />
            <input v-model="searchQuery" type="search" :placeholder="t('common.search')" :aria-label="t('common.search')" autofocus />
          </label>
          <div class="titlebar__search-results">
            <button v-for="item in searchResults" :key="item.id" type="button" class="titlebar__search-result" @click="goSearchResult(item.id)">
              <component :is="item.icon" :size="16" aria-hidden="true" />
              <span>{{ item.label }}</span>
              <ChevronRight :size="14" aria-hidden="true" />
            </button>
            <p v-if="!searchResults.length" class="titlebar__search-empty">{{ t('common.empty') }}</p>
          </div>
        </section>
      </div>
      <button class="titlebar__back" :title="t('titlebar.notifications')" :aria-label="t('titlebar.notifications')" @click="showToast(t('titlebar.notifications_pending'))"><Bell :size="18" /></button>
      <AndroidAccountCard v-if="isAndroid" />
      <AccountMenu v-else />
      <template v-if="!isMobile">
        <div class="titlebar__sep" />
        <div class="titlebar__controls">
          <button
            class="titlebar__btn"
            :title="t('titlebar.minimize')"
            @click="minimize"
          >
            <Minus :size="16" />
          </button>
          <button
            class="titlebar__btn"
            :title="maximized ? t('titlebar.restore') : t('titlebar.maximize')"
            @click="toggleMaximize"
          >
            <Copy v-if="maximized" :size="14" />
            <Square v-else :size="14" />
          </button>
          <button
            class="titlebar__btn titlebar__btn--close"
            :title="t('titlebar.close')"
            @click="close"
          >
            <X :size="16" />
          </button>
        </div>
      </template>
    </div>
  </header>
</template>

<style scoped>
.titlebar {
  display: flex;
  align-items: center;
  height: var(--copper-titlebar-h);
  flex-shrink: 0;
  padding-left: var(--copper-space-3);
  background: var(--copper-bg);
}

.titlebar__left {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: 100%;
  min-width: 0;
}

/*
 * 标题栏控件统一度量：32px 命中框 / 10px 圆角 / 18px 图标，
 * 底色默认透明，状态只靠微色阶。不再给返回与搜索套白色圆钮 ——
 * 那几个白点在浅色主题下会浮起来，和左导航的透明图标不是一套语言。
 */
.titlebar__back {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  padding: 0;
  flex: 0 0 32px;
  border: none;
  border-radius: var(--copper-radius-md);
  flex-shrink: 0;
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.titlebar__back:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.titlebar__title {
  font-size: var(--copper-font-size-md);
  font-weight: 700;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.titlebar__crumbs {
  display: flex;
  align-items: center;
  gap: 2px;
  min-width: 0;
}

/* 可点击段是 `button`、当前段是 `span`：两者都必须是同一种盒模型，
   否则 `height` 在 `span` 上失效、单行文本会贴着盒顶（表现为当前段文字偏上）。 */
.titlebar__crumb {
  display: inline-flex;
  align-items: center;
  height: 24px;
  padding: 0 var(--copper-space-2);
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
  line-height: 1;
  white-space: nowrap;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

button.titlebar__crumb {
  cursor: pointer;
}

button.titlebar__crumb:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.titlebar__crumb--current {
  color: var(--copper-text);
  font-weight: 700;
  cursor: default;
}

.titlebar__crumb-sep {
  flex-shrink: 0;
  color: var(--copper-text-disabled);
}

.titlebar__spacer {
  flex: 1;
  height: 100%;
}

.titlebar__actions {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: 100%;
  padding: 0 var(--copper-space-2);
}

/* Teleported page actions are outside their source component's scoped CSS. Keep
   their icon box stable so global button rules cannot collapse the glyph. */
.titlebar__actions :deep(svg) {
  width: 20px;
  height: 20px;
  flex: 0 0 20px;
}

/* 页面注入的操作与左右两侧控件同基准（32px），不另起第三套尺寸。 */
.titlebar__actions :deep(button) {
  min-width: 32px;
  min-height: 32px;
}

.titlebar__right > button:not(.titlebar__btn) {
  width: 32px;
  height: 32px;
}

.titlebar__right > button:not(.titlebar__btn) :deep(svg) {
  width: 18px;
  height: 18px;
}

/* Keep injected controls in normal flow; a fixed footer would cover MobileNav. */
.titlebar__back:disabled { opacity: .35; cursor: default; }
.titlebar__left { flex: 1; overflow: hidden; }
.titlebar__crumbs { overflow-x: auto; scrollbar-width: none; }
.titlebar__crumb { max-width: 240px; flex-shrink: 0; overflow: hidden; text-overflow: ellipsis; }
.titlebar__right, .titlebar__controls { flex-shrink: 0; }
.titlebar__spacer { flex: 0 0 24px; height: var(--copper-titlebar-h); }
/* Prevent desktop touch gestures from becoming WebView panning before native drag starts. */
.titlebar__spacer, .titlebar__title { touch-action: none; }
.titlebar__crumbs, .titlebar__actions { touch-action: pan-x; }
.titlebar__right { touch-action: manipulation; }
@media (max-width: 560px) {
  .titlebar { padding-left: max(4px, env(safe-area-inset-left)); }
  .titlebar__left { gap: 4px; min-width: 68px !important; }
  .titlebar__spacer { flex-basis: 8px; }
  .titlebar__right { padding-right: max(12px, env(safe-area-inset-right)); gap: 0; }
  .titlebar__right :deep(.account-menu__trigger) { gap: 4px; padding-right: 4px; }
  .titlebar__right :deep(.account-menu__label) { max-width: 64px; }
  .titlebar__sep { display: none; }
}
@media (max-width: 1050px) {
  .titlebar { flex-wrap: wrap; height: auto; min-height: var(--copper-titlebar-h); }
  .titlebar__left, .titlebar__right { height: var(--copper-titlebar-h); }
  .titlebar__left { min-width: 90px; }
  .titlebar__actions { order: 10; width: 100%; height: auto; overflow-x: auto; }
  .titlebar__actions:empty { display: none; }
  .titlebar__actions:not(:empty) { padding: 4px 8px; min-height: 40px; }
  .titlebar__btn { width: 32px; }
  .titlebar__right { gap: 2px; }
}
@media (prefers-reduced-motion: reduce) {
  .titlebar * { transition: none; }
}

/* Window controls remain independent of the page action row. */

.titlebar__right {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: 100%;
}

/* 启动异常入口：与标题栏其它控件同一触控尺寸，配色沿用语义色而非硬编码。 */
.titlebar__trouble {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  margin-right: var(--copper-space-1);
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-warning);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.titlebar__trouble:active {
  background: var(--copper-active);
}

.titlebar__sep {
  width: 1px;
  height: 16px;
  background: var(--copper-border);
}

.titlebar__controls {
  display: flex;
  height: 100%;
}

.titlebar__btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 46px;
  height: 100%;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.titlebar__btn:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.titlebar__btn--close:hover {
  background: var(--copper-danger);
  color: #ffffff;
}
/* Keep responsive overrides after base controls so narrow widths actually win. */
@media (max-width: 1050px) {
  .titlebar__btn { width: 32px; }
  .titlebar__right { gap: 2px; height: var(--copper-titlebar-h); }
}
@media (max-width: 560px) { .titlebar__sep { display: none; } }

.titlebar__search { position: relative; }
.titlebar__search-popover {
  position: absolute;
  z-index: 100;
  top: calc(100% + 10px);
  right: 0;
  width: min(360px, calc(100vw - 24px));
  padding: 12px;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface, var(--copper-bg));
  box-shadow: var(--copper-shadow-lg, 0 8px 28px rgb(0 0 0 / 18%));
  color: var(--copper-text);
}
.titlebar__search-field { display: flex; align-items: center; gap: 8px; height: 38px; padding: 0 10px; border-radius: var(--copper-radius-sm); background: var(--copper-hover); color: var(--copper-text-secondary); }
.titlebar__search-field input { width: 100%; min-width: 0; border: 0; outline: 0; background: transparent; color: var(--copper-text); font: inherit; }
.titlebar__search-results { max-height: 240px; overflow-y: auto; margin-top: 8px; }
.titlebar__search-result { display: flex; align-items: center; gap: 10px; width: 100%; min-height: 38px; padding: 0 8px; border: 0; border-radius: var(--copper-radius-sm); background: transparent; color: var(--copper-text); text-align: left; cursor: pointer; }
.titlebar__search-result:hover { background: var(--copper-hover); }
.titlebar__search-result span { flex: 1; }
.titlebar__search-empty { margin: 0; padding: 12px 8px; color: var(--copper-text-secondary); font-size: var(--copper-font-size-sm); }
</style>
