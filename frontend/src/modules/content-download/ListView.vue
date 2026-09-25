<script setup lang="ts">
// 内容下载列表页：搜索 / 来源·类型·版本·排序筛选（持久化）/ 分页 / 长条卡片。

import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import {
  Search,
  RefreshCw,
  ImageIcon,
  Layers,
  Sun,
  Puzzle,
  Download,
  User,
  Tag,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  LoaderCircle,
  SlidersHorizontal,
  RotateCcw,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import CoSelect from "../../components/ui/CoSelect.vue";
import TipsRotator from "../../components/TipsRotator.vue";
import { useSettings } from "../../composables/useSettings";
import ContentBadge from "./ContentBadge.vue";
import {
  sourceBadgeLabel,
  sourceBadgeTone,
  typeBadgeLabel,
  typeBadgeTone,
} from "./badges";
import {
  contentDownloadGameVersions,
  contentDownloadList,
  type ContentItem,
  type ContentSort,
  type ContentSource,
  type ContentType,
} from "./api";
import {
  contentType,
  error,
  gameVersion,
  hasLoadedVersions,
  hasMore,
  items,
  loading,
  markVersionsLoaded,
  PAGE_SIZE,
  page,
  scrollTop,
  searchInput,
  sort,
  source,
  total,
  versions,
  versionsError,
  versionsLoading,
} from "./listStore";

const { t } = useI18n();
const router = useRouter();
const settings = useSettings();

const MB_KEY = "module.content-download";

/** 筛选设置持久化 key（内核设置系统，全局共享）。 */
const SETTING_KEYS = {
  source: "content.filter.source",
  type: "content.filter.type",
  version: "content.filter.version",
  sort: "content.filter.sort",
} as const;

const SOURCES: Array<{ value: ContentSource | ""; label: string }> = [
  { value: "", label: `${MB_KEY}.sourceAll` },
  { value: "curseforge", label: `${MB_KEY}.sourceCurseforge` },
  { value: "lip", label: `${MB_KEY}.sourceLip` },
];

const TYPES: Array<{ value: ContentType | ""; label: string }> = [
  { value: "", label: `${MB_KEY}.typeAll` },
  { value: "behavior_pack", label: `${MB_KEY}.typeBehaviorPack` },
  { value: "texture_pack", label: `${MB_KEY}.typeTexturePack` },
  { value: "shader", label: `${MB_KEY}.typeShader` },
  { value: "ll_mod", label: `${MB_KEY}.typeLlMod` },
];

const SORTS: ContentSort[] = [
  "downloads_desc",
  "downloads_asc",
  "name_asc",
  "updated_desc",
];

const SORT_LABEL_KEY: Record<ContentSort, string> = {
  downloads_desc: `${MB_KEY}.sortDownloadsDesc`,
  downloads_asc: `${MB_KEY}.sortDownloadsAsc`,
  name_asc: `${MB_KEY}.sortNameAsc`,
  updated_desc: `${MB_KEY}.sortUpdatedDesc`,
};

let loadSeq = 0;
let hydrated = false;

const filterOpen = ref(false);
const filterRoot = ref<HTMLElement | null>(null);
const scrollerEl = ref<HTMLElement | null>(null);

/** 总页数（total 缺失或不确定时可退化为仅按 hasMore 累计）。 */
const totalPages = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)));

const sourceOptions = computed(() =>
  SOURCES.map((o) => ({ value: o.value, label: t(o.label) })),
);
const typeOptions = computed(() =>
  TYPES.map((o) => ({ value: o.value, label: t(o.label) })),
);
const sortOptions = computed(() =>
  SORTS.map((s) => ({ value: s, label: t(SORT_LABEL_KEY[s]) })),
);
const versionOptions = computed(() => [
  { value: "", label: t(`${MB_KEY}.versionAll`) },
  ...versions.value.map((v) => ({ value: v, label: v })),
]);

/** 当前生效的筛选数量（用于「筛选」按钮的角标提示）。 */
const activeFilterCount = computed(() => {
  let n = 0;
  if (source.value) n += 1;
  if (contentType.value) n += 1;
  if (gameVersion.value) n += 1;
  if (sort.value !== "downloads_desc") n += 1;
  return n;
});

/** 内容类型对应的图标。 */
function typeIcon(ct: ContentType) {
  switch (ct) {
    case "behavior_pack":
      return Puzzle;
    case "texture_pack":
      return Layers;
    case "shader":
      return Sun;
    default:
      return ImageIcon;
  }
}

/** 适配游戏版本范围展示（最低 ~ 最高）。 */
function versionRange(item: ContentItem): string {
  const lo = item.minGameVersion;
  const hi = item.maxGameVersion;
  if (lo && hi && lo !== hi) return `${lo} ~ ${hi}`;
  return lo || hi || t(`${MB_KEY}.unknownVersion`);
}

/** 列表加载：筛选条件与页码均取自 store。 */
async function fetchPage() {
  const seq = ++loadSeq;
  loading.value = true;
  error.value = null;
  try {
    const result = await contentDownloadList({
      source: source.value || undefined,
      contentType: contentType.value || undefined,
      search: searchInput.value.trim() || undefined,
      gameVersion: gameVersion.value || undefined,
      sort: sort.value,
      page: page.value,
    });
    if (seq !== loadSeq) return; // 丢弃过期响应
    items.value = result.items;
    hasMore.value = result.hasMore;
    total.value = result.total;
  } catch (e) {
    if (seq !== loadSeq) return;
    error.value = String(e);
  } finally {
    if (seq === loadSeq) loading.value = false;
  }
}

function search() {
  page.value = 0;
  void fetchPage();
}

function changePage(delta: number) {
  const next = page.value + delta;
  if (next < 0) return;
  if (delta > 0 && !hasMore.value) return;
  page.value = next;
  void fetchPage();
}

function openDetail(item: ContentItem) {
  const scroller = scrollerEl.value;
  if (scroller) scrollTop.value = scroller.scrollTop;
  void router.push(`/content/${encodeURIComponent(item.id)}`);
}

/** 版本列表：按需拉取一次并缓存于 store。 */
async function loadVersions(force = false) {
  if (versionsLoading.value) return;
  if (!force && hasLoadedVersions() && versions.value.length > 0) return;
  versionsLoading.value = true;
  versionsError.value = null;
  try {
    versions.value = await contentDownloadGameVersions();
    markVersionsLoaded();
  } catch (e) {
    versionsError.value = String(e);
  } finally {
    versionsLoading.value = false;
  }
}

function toggleFilter() {
  filterOpen.value = !filterOpen.value;
  if (filterOpen.value) void loadVersions();
}

function resetFilters() {
  source.value = "";
  contentType.value = "";
  gameVersion.value = "";
  sort.value = "downloads_desc";
}

function onDocumentPointerDown(event: PointerEvent) {
  if (!filterOpen.value) return;
  const root = filterRoot.value;
  if (root && !root.contains(event.target as Node)) filterOpen.value = false;
}

// 来源/类型/版本/排序变化即回到第一页并重新加载；返回列表时保留此前的数据与页码。
watch([source, contentType, gameVersion, sort], () => {
  page.value = 0;
  void fetchPage();
  void settings.setMany({
    [SETTING_KEYS.source]: source.value,
    [SETTING_KEYS.type]: contentType.value,
    [SETTING_KEYS.version]: gameVersion.value,
    [SETTING_KEYS.sort]: sort.value,
  });
});

onMounted(() => {
  document.addEventListener("pointerdown", onDocumentPointerDown);
  if (items.value.length === 0) {
    // 首次进入：读取持久化的筛选设置（仅首次，避免覆盖用户当前选择）。
    if (!hydrated) {
      hydrated = true;
      const savedSource = settings.get<ContentSource | "">(SETTING_KEYS.source, "");
      const savedType = settings.get<ContentType | "">(SETTING_KEYS.type, "");
      const savedVersion = settings.get<string>(SETTING_KEYS.version, "");
      const savedSort = settings.get<ContentSort>(SETTING_KEYS.sort, "downloads_desc");
      const changed =
        source.value !== savedSource ||
        contentType.value !== savedType ||
        gameVersion.value !== savedVersion ||
        sort.value !== savedSort;
      source.value = savedSource;
      contentType.value = savedType;
      gameVersion.value = savedVersion;
      sort.value = savedSort;
      // 恢复出的条件与默认值不同时，上面的 watch 已触发加载；否则手动拉取。
      if (!changed) void fetchPage();
    } else {
      void fetchPage();
    }
  } else {
    // 从详情页返回时数据已在 store 中，无需重新拉取；仅恢复滚动位置。
    requestAnimationFrame(() => {
      if (scrollerEl.value) scrollerEl.value.scrollTop = scrollTop.value;
    });
  }
});

onBeforeUnmount(() => {
  document.removeEventListener("pointerdown", onDocumentPointerDown);
});
</script>

<template>
  <div ref="scrollerEl" class="content-list">
    <!-- 刷新按钮注入全局标题栏操作区 -->
    <Teleport to="#copper-titlebar-actions">
      <button
        class="content-list__refresh"
        :title="t(`${MB_KEY}.refresh`)"
        :disabled="loading"
        @click="search"
      >
        <RefreshCw :size="15" :class="{ spin: loading }" />
      </button>
    </Teleport>

    <!-- 顶部操作栏：左搜索框，右筛选 + 搜索 -->
    <div class="content-list__toolbar">
      <div class="content-list__search">
        <Search :size="15" class="content-list__search-icon" />
        <input
          v-model="searchInput"
          class="content-list__search-input"
          :placeholder="t(`${MB_KEY}.searchPlaceholder`)"
          @keyup.enter="search"
        />
      </div>

      <div ref="filterRoot" class="content-list__filter-root">
        <button
          class="content-list__filter-btn"
          :class="{ active: filterOpen, 'has-value': activeFilterCount > 0 }"
          :title="t(`${MB_KEY}.filter`)"
          @click="toggleFilter"
        >
          <SlidersHorizontal :size="15" />
          <span>{{ t(`${MB_KEY}.filter`) }}</span>
          <span v-if="activeFilterCount > 0" class="content-list__filter-count">
            {{ activeFilterCount }}
          </span>
          <ChevronDown :size="14" :class="{ 'rotate-180': filterOpen }" />
        </button>

        <!-- 筛选面板 -->
        <div v-if="filterOpen" class="content-list__popover">
          <div class="content-list__popover-head">
            <span class="content-list__popover-title">{{ t(`${MB_KEY}.filterTitle`) }}</span>
            <button
              class="content-list__popover-reset"
              :disabled="activeFilterCount === 0"
              @click="resetFilters"
            >
              <RotateCcw :size="13" />
              {{ t(`${MB_KEY}.filterReset`) }}
            </button>
          </div>

          <label class="content-list__field">
            <span class="content-list__field-label">{{ t(`${MB_KEY}.source`) }}</span>
            <CoSelect v-model="source" :options="sourceOptions" />
          </label>

          <label class="content-list__field">
            <span class="content-list__field-label">{{ t(`${MB_KEY}.type`) }}</span>
            <CoSelect v-model="contentType" :options="typeOptions" />
          </label>

          <label class="content-list__field">
            <span class="content-list__field-label">{{ t(`${MB_KEY}.version`) }}</span>
            <CoSelect
              v-model="gameVersion"
              :options="versionOptions"
              :disabled="versionsLoading || !!versionsError"
            />
            <span v-if="versionsLoading" class="content-list__field-hint">
              <LoaderCircle :size="12" class="spin" />
            </span>
            <span v-else-if="versionsError" class="content-list__field-hint content-list__field-hint--error">
              {{ t(`${MB_KEY}.versionLoadFailed`) }}
              <button class="content-list__field-retry" @click="loadVersions(true)">
                {{ t(`${MB_KEY}.retry`) }}
              </button>
            </span>
          </label>

          <label class="content-list__field">
            <span class="content-list__field-label">{{ t(`${MB_KEY}.sort`) }}</span>
            <CoSelect v-model="sort" :options="sortOptions" />
          </label>
        </div>
      </div>

      <button class="content-list__search-btn" @click="search">
        {{ t(`${MB_KEY}.search`) }}
      </button>
    </div>

    <!-- 加载骨架 -->
    <div v-if="loading && items.length === 0" class="content-list__loading">
      <div class="content-list__rows">
        <div v-for="n in 8" :key="n" class="content-card content-card--skeleton">
          <div class="content-card__skeleton-thumb" />
          <div class="content-card__skeleton-body">
            <div class="content-card__skeleton-line" />
            <div class="content-card__skeleton-line content-card__skeleton-line--short" />
          </div>
        </div>
      </div>
      <!-- 内容拉取期间展示内核随机提示 -->
      <TipsRotator compact />
    </div>

    <!-- 错误态 -->
    <div v-else-if="error" class="content-list__state">
      <p class="content-list__state-text">{{ t(`${MB_KEY}.loadError`) }}</p>
      <button class="content-list__retry" @click="search">{{ t(`${MB_KEY}.retry`) }}</button>
    </div>

    <!-- 空态 -->
    <div v-else-if="items.length === 0" class="content-list__state">
      <p class="content-list__state-text">{{ t(`${MB_KEY}.noResult`) }}</p>
    </div>

    <!-- 长条卡片列表 -->
    <div v-else class="content-list__rows">
      <button
        v-for="item in items"
        :key="item.id"
        class="content-card"
        @click="openDetail(item)"
      >
        <div class="content-card__thumb">
          <img v-if="item.iconUrl" :src="item.iconUrl" :alt="item.name" loading="lazy" />
          <component :is="typeIcon(item.contentType)" v-else :size="24" class="content-card__thumb-fallback" />
        </div>

        <div class="content-card__main">
          <div class="content-card__title-row">
            <span class="content-card__name" :title="item.name">{{ item.name }}</span>
            <ContentBadge
              :tone="typeBadgeTone(item.contentType)"
              :label="typeBadgeLabel(item.contentType)"
            />
            <ContentBadge
              :tone="sourceBadgeTone(item.source)"
              :label="sourceBadgeLabel(item.source)"
            />
          </div>
          <div class="content-card__desc">{{ item.description }}</div>
        </div>

        <div class="content-card__meta">
          <span class="content-card__meta-item" :title="item.author || ''">
            <User :size="12" />
            <span class="content-card__meta-text">{{ item.author || t(`${MB_KEY}.unknownVersion`) }}</span>
          </span>
          <span class="content-card__meta-item">
            <Tag :size="12" />
            <span class="content-card__meta-text">{{ versionRange(item) }}</span>
          </span>
          <span class="content-card__meta-item">
            <Download :size="12" />
            <span class="content-card__meta-text">{{ item.downloadCount.toLocaleString() }}</span>
          </span>
        </div>
      </button>
    </div>

    <!-- 分页 -->
    <footer v-if="items.length > 0" class="content-list__pager">
      <button
        class="content-list__pager-btn"
        :disabled="page === 0 || loading"
        @click="changePage(-1)"
      >
        <ChevronLeft :size="15" />
        {{ t(`${MB_KEY}.prev`) }}
      </button>
      <span class="content-list__pager-state">
        <LoaderCircle v-if="loading" :size="14" class="spin" />
        <template v-else>
          <span class="content-list__page-no">{{ t(`${MB_KEY}.pageInfo`, { current: page + 1, total: totalPages }) }}</span>
          <span class="content-list__pager-status">{{ hasMore ? t(`${MB_KEY}.hasMore`) : t(`${MB_KEY}.noMore`) }}</span>
        </template>
      </span>
      <button
        class="content-list__pager-btn"
        :disabled="!hasMore || loading"
        @click="changePage(1)"
      >
        {{ t(`${MB_KEY}.next`) }}
        <ChevronRight :size="15" />
      </button>
    </footer>
  </div>
</template>

<style scoped>
.content-list {
  height: 100%;
  padding: var(--copper-space-5);
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}

.content-list__refresh {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.content-list__refresh:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

/* ---------------------------------------------------------------- 顶部操作栏 */
.content-list__toolbar {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  flex-shrink: 0;
}

.content-list__search {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  transition: border-color var(--copper-duration-fast) var(--copper-easing);
}

.content-list__search:focus-within {
  border-color: var(--copper-accent);
}

.content-list__search-icon {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
}

.content-list__search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
}

.content-list__filter-root {
  position: relative;
  flex-shrink: 0;
}

.content-list__filter-btn {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-1);
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition:
    border-color var(--copper-duration-fast) var(--copper-easing),
    background-color var(--copper-duration-fast) var(--copper-easing);
}

.content-list__filter-btn:hover,
.content-list__filter-btn.active {
  border-color: var(--copper-accent);
}

.content-list__filter-btn.has-value {
  color: var(--copper-accent);
}

.content-list__filter-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  color: var(--copper-on-accent, #fff);
  font-size: 10px;
  line-height: 1;
}

.content-list__popover {
  position: absolute;
  top: calc(100% + var(--copper-space-2));
  right: 0;
  z-index: 20;
  width: 300px;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  padding: var(--copper-space-4);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.18);
}

.content-list__popover-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.content-list__popover-title {
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

.content-list__popover-reset {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  transition: color var(--copper-duration-fast) var(--copper-easing);
}

.content-list__popover-reset:hover:not(:disabled) {
  color: var(--copper-accent);
}

.content-list__popover-reset:disabled {
  opacity: 0.4;
  cursor: default;
}

.content-list__field {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.content-list__field-label {
  flex-shrink: 0;
  width: 60px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.content-list__field :deep(.co-select) {
  flex: 1;
  min-width: 0;
}

.content-list__field :deep(.co-select select) {
  width: 100%;
  height: var(--copper-control-h-sm);
  font-size: var(--copper-font-size-sm);
}

.content-list__field-hint {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.content-list__field-hint--error {
  color: var(--copper-danger, #e5484d);
  flex-shrink: 0;
}

.content-list__field-retry {
  border: none;
  background: transparent;
  color: var(--copper-accent);
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  text-decoration: underline;
}

.content-list__search-btn {
  flex-shrink: 0;
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-4);
  border: none;
  border-radius: var(--copper-radius-md);
  background: var(--copper-accent);
  color: var(--copper-on-accent, #fff);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition: opacity var(--copper-duration-fast) var(--copper-easing);
}

.content-list__search-btn:hover {
  opacity: 0.9;
}

/* ---------------------------------------------------------------- 长条卡片 */
.content-list__rows {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  flex: 1;
  margin-top: var(--copper-space-4);
}

.content-list__state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-6);
}

.content-list__state-text {
  color: var(--copper-text-secondary);
}

.content-list__retry,
.content-list__pager-btn {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-1);
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.content-list__retry:hover,
.content-list__pager-btn:hover:not(:disabled) {
  background: var(--copper-surface-2);
}

.content-list__pager-btn:disabled {
  opacity: 0.5;
  cursor: default;
}

.content-card {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  width: 100%;
  text-align: left;
  padding: var(--copper-space-3);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  cursor: pointer;
  transition:
    transform var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.content-card:hover {
  border-color: color-mix(in srgb, var(--copper-accent) 45%, var(--copper-border));
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.12);
}

.content-card__thumb {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 52px;
  height: 52px;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  overflow: hidden;
}

.content-card__thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.content-card__thumb-fallback {
  color: var(--copper-text-secondary);
}

.content-card__main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.content-card__title-row {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  min-width: 0;
}

.content-card__name {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.content-card__badge {
  flex-shrink: 0;
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
  color: var(--copper-accent);
  font-size: var(--copper-font-size-xs);
  line-height: 1.6;
}

.content-card__desc {
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.content-card__meta {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 4px;
  min-width: 130px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.content-card__meta-item {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  max-width: 180px;
}

.content-card__meta-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.content-card--skeleton .content-card__thumb {
  background: var(--copper-surface-3);
}

.content-card--skeleton .content-card__skeleton-thumb {
  flex-shrink: 0;
  width: 52px;
  height: 52px;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-3);
}

.content-card__skeleton-body {
  flex: 1;
}

.content-card__skeleton-line {
  height: 12px;
  margin-top: var(--copper-space-2);
  border-radius: var(--copper-radius-xs);
  background: var(--copper-surface-3);
}

.content-card__skeleton-line:first-child {
  margin-top: 0;
}

.content-card__skeleton-line--short {
  width: 60%;
}

/* ---------------------------------------------------------------- 分页 */
.content-list__pager {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-3);
  padding-top: var(--copper-space-4);
}

.content-list__pager-state {
  display: flex;
  align-items: center;
  flex-direction: column;
  gap: 2px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  min-width: 130px;
  justify-content: center;
}

.content-list__page-no {
  font-weight: 600;
  color: var(--copper-text);
}

.spin {
  animation: spin 1.2s linear infinite;
}

.rotate-180 {
  transform: rotate(180deg);
  transition: transform var(--copper-duration-fast) var(--copper-easing);
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
