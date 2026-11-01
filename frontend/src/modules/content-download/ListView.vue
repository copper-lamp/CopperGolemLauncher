<script setup lang="ts">
// 内容下载列表页：左侧结果列表 + 右侧「搜索 / 分页 / 平铺筛选」。
//
// 右栏结构照设计稿（参考 levimc.org/mods 的信息密度）：
// - 最上面两个组件**各自独立、不套卡片**：搜索框、搜索按钮；
// - 下面是一张卡片，卡片顶部是翻页（全圆形按钮），再往下是**平铺筛选栏**；
// - 筛选不再是下拉框：每个维度是一组胶囊 chip，纵向排列，整体可滚动。
//
// 视觉遵守本轮规则：无描边，靠底色阶差 + 阴影分层。
//
// 分页依据是后端返回的 `totalPages`（各来源都给了真数，见 listStore 注释），
// 不再用“已探索页数”猜末页。

import { computed, onMounted, ref, watch } from "vue";
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
  RotateCcw,
  LoaderCircle,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import CoBadge from "../../components/ui/CoBadge.vue";
import CoPagination from "../../components/ui/CoPagination.vue";
import CoPage from "../../components/layout/CoPage.vue";
import TipsRotator from "../../components/TipsRotator.vue";
import { usePlatform } from "../../composables/usePlatform";
import { useSettings } from "../../composables/useSettings";
import {
  sourceBadgeLabel,
  sourceBadgeTone,
  typeBadgeLabel,
  typeBadgeTone,
} from "./badges";
import {
  contentDownloadGameVersions,
  contentDownloadList,
  contentDownloadTags,
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
  page,
  scrollTop,
  searchInput,
  sort,
  source,
  tag,
  total,
  totalPages,
  versions,
  versionsError,
  versionsLoading,
} from "./listStore";

const { t } = useI18n();
const router = useRouter();
const settings = useSettings();
const { isAndroid } = usePlatform();

const MB_KEY = "module.content-download";

/** 筛选设置持久化 key（内核设置系统，全局共享）。 */
const SETTING_KEYS = {
  source: "content.filter.source",
  type: "content.filter.type",
  version: "content.filter.version",
  sort: "content.filter.sort",
  tag: "content.filter.tag",
} as const;

/**
 * 来源选项。
 *
 * `lla`（安卓 LL 模组 / LeviModHub 目录）仅在 Android 出现：桌面端 lip
 * 装不了它，安卓端 lipd 又不可用，两个来源在各自平台互斥。
 */
const SOURCES: Array<{ value: ContentSource | ""; label: string }> = [
  { value: "", label: MB_KEY + ".sourceAll" },
  { value: "curseforge", label: MB_KEY + ".sourceCurseforge" },
  isAndroid.value
    ? { value: "lla", label: MB_KEY + ".sourceLla" }
    : { value: "lip", label: MB_KEY + ".sourceLip" },
];

const TYPES: Array<{ value: ContentType | ""; label: string }> = [
  { value: "", label: MB_KEY + ".typeAll" },
  { value: "behavior_pack", label: MB_KEY + ".typeBehaviorPack" },
  { value: "texture_pack", label: MB_KEY + ".typeTexturePack" },
  { value: "shader", label: MB_KEY + ".typeShader" },
  { value: "ll_mod", label: MB_KEY + ".typeLlMod" },
];

const SORTS: ContentSort[] = [
  "downloads_desc",
  "downloads_asc",
  "name_asc",
  "updated_desc",
];

const SORT_LABEL_KEY: Record<ContentSort, string> = {
  downloads_desc: MB_KEY + ".sortDownloadsDesc",
  downloads_asc: MB_KEY + ".sortDownloadsAsc",
  name_asc: MB_KEY + ".sortNameAsc",
  updated_desc: MB_KEY + ".sortUpdatedDesc",
};

let loadSeq = 0;
let hydrated = false;

const pageRef = ref<InstanceType<typeof CoPage> | null>(null);
/** 左侧列表自己的滚动容器（整页不滚动，右栏才能顶满高度且不跟滚）。 */
const resultsRef = ref<HTMLElement | null>(null);
const tagOptions = ref<string[]>([]);

function scrollBody(): HTMLElement | null {
  return resultsRef.value ?? pageRef.value?.bodyEl ?? null;
}

const pagerLabels = computed(() => ({
  summary: (current: number, all: number) =>
    t(MB_KEY + ".pageInfo", { current, total: all }),
  prev: t(MB_KEY + ".prev"),
  next: t(MB_KEY + ".next"),
  nav: t(MB_KEY + ".pageNav"),
  goto: (n: number) => t(MB_KEY + ".pageGoto", { page: n }),
}));

const sourceOptions = computed(() =>
  SOURCES.map((o) => ({ value: o.value, label: t(o.label) })),
);
const typeOptions = computed(() =>
  TYPES.map((o) => ({ value: o.value, label: t(o.label) })),
);
const sortOptions = computed(() =>
  SORTS.map((s) => ({ value: s, label: t(SORT_LABEL_KEY[s]) })),
);

/** 当前生效的筛选数量（用于「重置」的可用态）。 */
const activeFilterCount = computed(() => {
  let n = 0;
  if (source.value) n += 1;
  if (contentType.value) n += 1;
  if (gameVersion.value) n += 1;
  if (tag.value) n += 1;
  if (sort.value !== "downloads_desc") n += 1;
  return n;
});

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

function versionRange(item: ContentItem): string {
  const lo = item.minGameVersion;
  const hi = item.maxGameVersion;
  if (lo && hi && lo !== hi) return lo + " ~ " + hi;
  return lo || hi || t(MB_KEY + ".unknownVersion");
}

/** 卡片上最多展示 3 个标签，其余折叠成「+n」。 */
function visibleTags(item: ContentItem): string[] {
  return (item.categories ?? []).slice(0, 3);
}

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
      tag: tag.value || undefined,
      sort: sort.value,
      page: page.value,
    });
    if (seq !== loadSeq) return;
    items.value = result.items;
    hasMore.value = result.hasMore;
    total.value = result.total;
    // 总页数由后端算好：各来源的 total 语义已经统一，不再需要“已探索页数”。
    totalPages.value = Math.max(1, result.totalPages || 1);
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

function jumpPage(target: number) {
  if (target < 0 || target >= totalPages.value || target === page.value) return;
  page.value = target;
  const scroller = scrollBody();
  if (scroller) scroller.scrollTop = 0;
  void fetchPage();
}

function openDetail(item: ContentItem) {
  const scroller = scrollBody();
  if (scroller) scrollTop.value = scroller.scrollTop;
  void router.push("/content/" + encodeURIComponent(item.id));
}

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

/** 标签表按来源变化：桌面端 lip 的标签与 CurseForge 的类别是两套词表。 */
async function loadTags() {
  try {
    tagOptions.value = await contentDownloadTags(source.value || undefined);
  } catch {
    tagOptions.value = [];
  }
  // 当前标签不在新词表里（例如切换了来源）时清掉，避免“选了但列表为空”。
  if (tag.value && !tagOptions.value.includes(tag.value)) tag.value = "";
}

function resetFilters() {
  source.value = "";
  contentType.value = "";
  gameVersion.value = "";
  tag.value = "";
  sort.value = "downloads_desc";
}

watch([source, contentType, gameVersion, sort, tag], () => {
  page.value = 0;
  void fetchPage();
  void settings.setMany({
    [SETTING_KEYS.source]: source.value,
    [SETTING_KEYS.type]: contentType.value,
    [SETTING_KEYS.version]: gameVersion.value,
    [SETTING_KEYS.sort]: sort.value,
    [SETTING_KEYS.tag]: tag.value,
  });
});

watch(source, () => void loadTags());

onMounted(() => {
  void loadVersions();
  void loadTags();
  if (items.value.length === 0) {
    if (!hydrated) {
      hydrated = true;
      const savedSource = settings.get<ContentSource | "">(SETTING_KEYS.source, "");
      const savedType = settings.get<ContentType | "">(SETTING_KEYS.type, "");
      const savedVersion = settings.get<string>(SETTING_KEYS.version, "");
      const savedSort = settings.get<ContentSort>(SETTING_KEYS.sort, "downloads_desc");
      const savedTag = settings.get<string>(SETTING_KEYS.tag, "");
      const changed =
        source.value !== savedSource ||
        contentType.value !== savedType ||
        gameVersion.value !== savedVersion ||
        sort.value !== savedSort;
      source.value = SOURCES.some((option) => option.value === savedSource)
        ? savedSource
        : "";
      contentType.value = savedType;
      gameVersion.value = savedVersion;
      sort.value = savedSort;
      tag.value = savedTag;
      if (!changed) void fetchPage();
    } else {
      void fetchPage();
    }
  } else {
    requestAnimationFrame(() => {
      const scroller = scrollBody();
      if (scroller) scroller.scrollTop = scrollTop.value;
    });
  }
});
</script>
<template>
  <CoPage ref="pageRef" :scroll="false">
    <Teleport defer to="#copper-titlebar-actions">
      <button
        class="list-page__refresh"
        :title="t(MB_KEY + '.refresh')"
        :disabled="loading"
        @click="search"
      >
        <RefreshCw :size="15" :class="{ spin: loading }" />
      </button>
    </Teleport>

    <div class="list-page">
      <!-- 左：结果列表 -->
      <section ref="resultsRef" class="list-page__results">
        <div v-if="loading && items.length === 0" class="list-page__state">
          <div v-for="n in 6" :key="n" class="list-page__skeleton">
            <div class="list-page__skeleton-thumb" />
            <div class="list-page__skeleton-lines">
              <div class="list-page__skeleton-line" />
              <div class="list-page__skeleton-line list-page__skeleton-line--short" />
            </div>
          </div>
          <TipsRotator compact />
        </div>

        <div v-else-if="error" class="list-page__state">
          <p class="list-page__state-text">{{ t(MB_KEY + ".loadError") }}</p>
          <button class="list-page__retry" @click="search">{{ t(MB_KEY + ".retry") }}</button>
        </div>

        <div v-else-if="items.length === 0" class="list-page__state">
          <p class="list-page__state-text">{{ t(MB_KEY + ".noResult") }}</p>
        </div>

        <div v-else class="list-page__rows">
          <button
            v-for="item in items"
            :key="item.id"
            class="content-card"
            @click="openDetail(item)"
          >
            <div class="content-card__thumb">
              <img v-if="item.iconUrl" :src="item.iconUrl" :alt="item.name" loading="lazy" />
              <component
                :is="typeIcon(item.contentType)"
                v-else
                :size="24"
                class="content-card__thumb-fallback"
              />
            </div>

            <div class="content-card__main">
              <div class="content-card__title-row">
                <span class="content-card__name" :title="item.name">{{ item.name }}</span>
                <CoBadge
                  :tone="typeBadgeTone(item.contentType)"
                  :label="typeBadgeLabel(item.contentType, item.source)"
                />
                <CoBadge
                  :tone="sourceBadgeTone(item.source)"
                  :label="sourceBadgeLabel(item.source)"
                />
              </div>
              <div class="content-card__desc">{{ item.description }}</div>
              <div v-if="visibleTags(item).length" class="content-card__tags">
                <span v-for="name in visibleTags(item)" :key="name" class="content-card__tag">
                  {{ name }}
                </span>
                <span v-if="(item.categories?.length ?? 0) > 3" class="content-card__tag">
                  +{{ (item.categories?.length ?? 0) - 3 }}
                </span>
              </div>
            </div>

            <div class="content-card__meta">
              <span class="content-card__meta-item" :title="item.author || ''">
                <User :size="12" />
                <span class="content-card__meta-text">
                  {{ item.author || t(MB_KEY + ".unknownVersion") }}
                </span>
              </span>
              <span class="content-card__meta-item">
                <Download :size="12" />
                <span class="content-card__meta-text">{{ item.downloadCount.toLocaleString() }}</span>
              </span>
              <span class="content-card__meta-item">
                <span class="content-card__meta-text">{{ versionRange(item) }}</span>
              </span>
            </div>
          </button>
        </div>
      </section>

      <!-- 右：搜索（独立）+ 按钮（独立）+ 卡片（翻页 / 平铺筛选） -->
      <aside class="rail">
        <div class="rail__search">
          <Search :size="15" class="rail__search-icon" />
          <input
            v-model="searchInput"
            class="rail__search-input"
            :placeholder="t(MB_KEY + '.searchPlaceholder')"
            @keyup.enter="search"
          />
        </div>

        <button class="rail__go" @click="search">{{ t(MB_KEY + ".search") }}</button>

        <div class="rail__card">
          <div class="rail__pager">
            <CoPagination
              shape="circle"
              :page="page"
              :total-pages="totalPages"
              :loading="loading"
              :labels="pagerLabels"
              @change="jumpPage"
            />
          </div>

          <div class="rail__filters">
            <div class="filter-group">
              <span class="filter-group__label">{{ t(MB_KEY + ".source") }}</span>
              <div class="filter-group__chips">
                <button
                  v-for="opt in sourceOptions"
                  :key="opt.value"
                  :class="['chip', { 'chip--active': source === opt.value }]"
                  @click="source = opt.value"
                >
                  {{ opt.label }}
                </button>
              </div>
            </div>

            <div class="filter-group">
              <span class="filter-group__label">{{ t(MB_KEY + ".type") }}</span>
              <div class="filter-group__chips">
                <button
                  v-for="opt in typeOptions"
                  :key="opt.value"
                  :class="['chip', { 'chip--active': contentType === opt.value }]"
                  @click="contentType = opt.value"
                >
                  {{ opt.label }}
                </button>
              </div>
            </div>

            <div class="filter-group">
              <span class="filter-group__label">{{ t(MB_KEY + ".version") }}</span>
              <div v-if="versionsLoading" class="filter-group__hint">
                <LoaderCircle :size="12" class="spin" />
              </div>
              <div v-else-if="versionsError" class="filter-group__hint filter-group__hint--error">
                {{ t(MB_KEY + ".versionLoadFailed") }}
                <button class="filter-group__retry" @click="loadVersions(true)">
                  {{ t(MB_KEY + ".retry") }}
                </button>
              </div>
              <div v-else class="filter-group__chips">
                <button
                  :class="['chip', { 'chip--active': gameVersion === '' }]"
                  @click="gameVersion = ''"
                >
                  {{ t(MB_KEY + ".versionAll") }}
                </button>
                <button
                  v-for="v in versions"
                  :key="v"
                  :class="['chip', { 'chip--active': gameVersion === v }]"
                  @click="gameVersion = v"
                >
                  {{ v }}
                </button>
              </div>
            </div>

            <div class="filter-group">
              <span class="filter-group__label">{{ t(MB_KEY + ".sort") }}</span>
              <div class="filter-group__chips">
                <button
                  v-for="opt in sortOptions"
                  :key="opt.value"
                  :class="['chip', { 'chip--active': sort === opt.value }]"
                  @click="sort = opt.value"
                >
                  {{ opt.label }}
                </button>
              </div>
            </div>

            <div v-if="tagOptions.length" class="filter-group">
              <span class="filter-group__label">{{ t(MB_KEY + ".tag") }}</span>
              <div class="filter-group__chips">
                <button :class="['chip', { 'chip--active': tag === '' }]" @click="tag = ''">
                  {{ t(MB_KEY + ".tagAll") }}
                </button>
                <button
                  v-for="name in tagOptions"
                  :key="name"
                  :class="['chip', 'chip--tag', { 'chip--active': tag === name }]"
                  @click="tag = tag === name ? '' : name"
                >
                  {{ name }}
                </button>
              </div>
            </div>
          </div>

          <button class="rail__reset" :disabled="activeFilterCount === 0" @click="resetFilters">
            <RotateCcw :size="12" />
            {{ t(MB_KEY + ".filterReset") }}
          </button>
        </div>
      </aside>
    </div>
  </CoPage>
</template>
<style scoped>
.list-page {
  display: grid;
  /* 7:3：列表是主区，右栏只放搜索与筛选。 */
  grid-template-columns: minmax(0, 7fr) minmax(240px, 3fr);
  gap: var(--copper-space-4);
  /* 顶满可用高度：右栏不跟随页面滚动，列表自己滚。 */
  height: 100%;
  min-height: 0;
  align-items: stretch;
}

/* 左：列表自己的滚动容器。滚动条贴内容区右缘，不推动右栏。 */
.list-page__results {
  min-width: 0;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding-right: 2px;
}

.list-page__refresh {
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

.list-page__refresh:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

/* ---------------------------------------------------------------- 左：列表 */
.list-page__rows {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.list-page__state {
  min-height: 40vh;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-6);
}

.list-page__state-text {
  color: var(--copper-text-secondary);
}

.list-page__retry {
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
}

.list-page__skeleton {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  width: 100%;
  padding: var(--copper-space-3);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
}

.list-page__skeleton-thumb {
  width: 52px;
  height: 52px;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-3);
}

.list-page__skeleton-lines {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.list-page__skeleton-line {
  height: 12px;
  border-radius: var(--copper-radius-xs);
  background: var(--copper-surface-3);
}

.list-page__skeleton-line--short {
  width: 60%;
}

/* 卡片：无描边，靠底色 + 阴影分层。 */
.content-card {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  width: 100%;
  text-align: left;
  padding: var(--copper-space-3);
  background: var(--copper-surface);
  border: none;
  border-radius: var(--copper-radius-lg);
  box-shadow: var(--copper-shadow-subtle);
  cursor: pointer;
  transition:
    transform var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.content-card:hover {
  box-shadow: var(--copper-shadow);
  transform: translateY(-1px);
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
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.content-card__desc {
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.content-card__tags {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 2px;
}

.content-card__tag {
  padding: 1px 7px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-size: 10px;
  text-transform: capitalize;
}

.content-card__meta {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 4px;
  min-width: 120px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.content-card__meta-item {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  max-width: 170px;
}

.content-card__meta-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ---------------------------------------------------------------- 右栏 */
.rail {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  min-width: 0;
  min-height: 0;
  /* 顶满高度：卡片撑满剩余空间，筛选在自己内部滚动。 */
  height: 100%;
}

/* 搜索框：独立组件，不套卡片。 */
.rail__search {
  position: relative;
  display: flex;
  align-items: center;
  height: 44px;
  padding: 0 var(--copper-space-3) 0 38px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow-subtle);
}

.rail__search:focus-within {
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--copper-accent) 40%, transparent);
}

.rail__search-icon {
  position: absolute;
  left: var(--copper-space-3);
  color: var(--copper-text-secondary);
  pointer-events: none;
}

.rail__search-input {
  width: 100%;
  height: 100%;
  border: none;
  outline: none;
  background: transparent;
  color: var(--copper-text);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
}

/* 搜索按钮：同样是独立组件。 */
.rail__go {
  height: 40px;
  border: none;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.rail__go:hover {
  background: color-mix(in srgb, var(--copper-accent) 88%, #fff);
}

/* 卡片：顶部翻页，下面平铺筛选。 */
.rail__card {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow-subtle);
}

.rail__pager :deep(.co-pager) {
  padding-top: 0;
  align-items: flex-start;
  gap: var(--copper-space-1);
}

.rail__pager :deep(.co-pager__summary) {
  font-size: 10px;
}

/* 筛选栏：平铺，内容多了在卡片内滚动。 */
.rail__filters {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  overflow-y: auto;
  overscroll-behavior: contain;
  padding-right: 2px;
}

.filter-group {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
  min-width: 0;
}

.filter-group__label {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
}

.filter-group__chips {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.chip {
  height: 26px;
  padding: 0 10px;
  border: none;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.chip:hover {
  background: var(--copper-surface-3);
  color: var(--copper-text);
}

.chip--active {
  background: color-mix(in srgb, var(--copper-accent) 20%, transparent);
  color: color-mix(in srgb, var(--copper-accent) 75%, var(--copper-text));
  font-weight: 600;
}

.chip--tag {
  text-transform: capitalize;
}

.filter-group__hint {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.filter-group__hint--error {
  color: var(--copper-danger);
}

.filter-group__retry {
  border: none;
  background: transparent;
  color: var(--copper-accent);
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  text-decoration: underline;
}

.rail__reset {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 4px;
  height: 30px;
  border: none;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
}

.rail__reset:hover:not(:disabled) {
  color: var(--copper-text);
  background: var(--copper-hover);
}

.rail__reset:disabled {
  opacity: 0.4;
  cursor: default;
}

@container (max-width: 860px) {
  .list-page {
    grid-template-columns: minmax(0, 1fr);
    height: auto;
    overflow-y: auto;
  }

  .list-page__results {
    overflow: visible;
  }

  .rail {
    order: -1;
    height: auto;
  }

  .rail__filters {
    overflow: visible;
  }
}

.spin {
  animation: spin 1.2s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>