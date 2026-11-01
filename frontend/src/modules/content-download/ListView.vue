<script setup lang="ts">
// 内容下载列表页：长条卡片列表 + 右栏「搜索 / 筛选 / 分页」。
//
// 布局（V3 骨架）：主栏只放结果列表，搜索框、来源·类型·版本·排序筛选（持久化）
// 与分页全部收进右侧副栏。窄容器折叠时副栏排到列表之前（stack-aside-first），
// 因为筛选必须先于结果出现。

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
  Tag,
  LoaderCircle,
  RotateCcw,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import CoBadge from "../../components/ui/CoBadge.vue";
import CoPagination from "../../components/ui/CoPagination.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import CoPage from "../../components/layout/CoPage.vue";
import CoSplitLayout from "../../components/layout/CoSplitLayout.vue";
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
  type ContentItem,
  type ContentSort,
  type ContentSource,
  type ContentType,
} from "./api";
import {
  advanceKnownPages,
  contentType,
  error,
  gameVersion,
  hasLoadedVersions,
  hasMore,
  items,
  knownPages,
  loading,
  markVersionsLoaded,
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
const { isAndroid } = usePlatform();

const MB_KEY = "module.content-download";

/** 筛选设置持久化 key（内核设置系统，全局共享）。 */
const SETTING_KEYS = {
  source: "content.filter.source",
  type: "content.filter.type",
  version: "content.filter.version",
  sort: "content.filter.sort",
} as const;

/**
 * 来源选项。
 *
 * `lla`（安卓 LL 模组 / LeviModHub 目录）仅在 Android 出现：桌面端 lip
 * 装不了它，安卓端 lipd 又不可用，两个来源在各自平台互斥（见
 * docs/模块/内容下载/设计.md 安卓章节）。
 */
const SOURCES: Array<{ value: ContentSource | ""; label: string }> = [
  { value: "", label: `${MB_KEY}.sourceAll` },
  { value: "curseforge", label: `${MB_KEY}.sourceCurseforge` },
  isAndroid.value
    ? { value: "lla", label: `${MB_KEY}.sourceLla` }
    : { value: "lip", label: `${MB_KEY}.sourceLip` },
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

/** 页面骨架实例：列表的滚动位置由 CoPage 的正文容器承载。 */
const pageRef = ref<InstanceType<typeof CoPage> | null>(null);

function scrollBody(): HTMLElement | null {
  return pageRef.value?.bodyEl ?? null;
}

/**
 * 分页文案：由本模块的 i18n 命名空间注入，CoPagination 本身不认识「内容下载」。
 * 做成 computed 是因为语言切换 / 动态命名空间加载后必须跟着变。
 */
const pagerLabels = computed(() => ({
  summary: (current: number, all: number) =>
    t(`${MB_KEY}.pageInfo`, { current, total: all }),
  prev: t(`${MB_KEY}.prev`),
  next: t(`${MB_KEY}.next`),
  nav: t(`${MB_KEY}.pageNav`),
  goto: (n: number) => t(`${MB_KEY}.pageGoto`, { page: n }),
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
    // 已探索页数只能从 `hasMore` 推（内核三条路径的 `total` 语义各不相同，
    // 默认的 mixed 路径给的是累加假值，拿它算页数会恒为 1 而让分页整条消失）。
    advanceKnownPages(page.value, result.hasMore);
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

/**
 * 跳到指定页（0 基）。
 *
 * 分页组件已经吃掉了越界、重复点击与 loading 期间点击，这里只负责落地状态并拉取。
 * 用 `page.value = target` 而不是相对增减：面包屑页码是**绝对跳转**，相对增减要再算一遍
 * 边界，两处都算就迟早会对不上。
 */
function jumpPage(target: number) {
  if (target < 0 || target >= knownPages.value || target === page.value) return;
  page.value = target;
  // 翻页后滚回顶部，否则用户停在新页面的半截位置，还以为没换页。
  const scroller = scrollBody();
  if (scroller) scroller.scrollTop = 0;
  void fetchPage();
}

function openDetail(item: ContentItem) {
  const scroller = scrollBody();
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

function resetFilters() {
  source.value = "";
  contentType.value = "";
  gameVersion.value = "";
  sort.value = "downloads_desc";
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
  // 版本下拉在右栏常驻，进入页面就拉一次；失败可点重试。
  void loadVersions();
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
      // 恢复的来源必须在本平台可选：设置是全局持久化的，同一份数据可能
      // 来自另一平台（安卓存的 `lla` 在桌面上没有 lipd 可用，反之亦然）。
      source.value = SOURCES.some((option) => option.value === savedSource)
        ? savedSource
        : "";
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
      const scroller = scrollBody();
      if (scroller) scroller.scrollTop = scrollTop.value;
    });
  }
});

</script>

<template>
  <CoPage ref="pageRef">
    <CoSplitLayout aside-position="end" stack-aside-first>
      <Teleport defer to="#copper-titlebar-actions">
        <button
          class="content-list__refresh"
          :title="t(`${MB_KEY}.refresh`)"
          :disabled="loading"
          @click="search"
        >
          <RefreshCw :size="15" :class="{ spin: loading }" />
        </button>
      </Teleport>
      <div class="content-list">
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

      </div>

      <template #aside>
        <aside class="content-filters">
          <div class="content-filters__search">
            <Search :size="15" class="content-filters__search-icon" />
            <input
              v-model="searchInput"
              class="content-filters__search-input"
              :placeholder="t(`${MB_KEY}.searchPlaceholder`)"
              @keyup.enter="search"
            />
          </div>
          <button class="content-filters__search-btn" @click="search">
            {{ t(`${MB_KEY}.search`) }}
          </button>

          <div class="content-filters__head">
            <span class="content-filters__title">{{ t(`${MB_KEY}.filterTitle`) }}</span>
            <button
              class="content-filters__reset"
              :disabled="activeFilterCount === 0"
              @click="resetFilters"
            >
              <RotateCcw :size="12" />
              {{ t(`${MB_KEY}.filterReset`) }}
            </button>
          </div>

          <div class="content-filters__fields">
            <label class="content-filters__field">
              <span class="content-filters__field-label">{{ t(`${MB_KEY}.source`) }}</span>
              <CoSelect v-model="source" :options="sourceOptions" />
            </label>

            <label class="content-filters__field">
              <span class="content-filters__field-label">{{ t(`${MB_KEY}.type`) }}</span>
              <CoSelect v-model="contentType" :options="typeOptions" />
            </label>

            <label class="content-filters__field">
              <span class="content-filters__field-label">{{ t(`${MB_KEY}.version`) }}</span>
              <CoSelect
                v-model="gameVersion"
                :options="versionOptions"
                :disabled="versionsLoading || !!versionsError"
              />
              <span v-if="versionsLoading" class="content-filters__field-hint">
                <LoaderCircle :size="12" class="spin" />
              </span>
              <span
                v-else-if="versionsError"
                class="content-filters__field-hint content-filters__field-hint--error"
              >
                {{ t(`${MB_KEY}.versionLoadFailed`) }}
                <button class="content-filters__field-retry" @click="loadVersions(true)">
                  {{ t(`${MB_KEY}.retry`) }}
                </button>
              </span>
            </label>

            <label class="content-filters__field">
              <span class="content-filters__field-label">{{ t(`${MB_KEY}.sort`) }}</span>
              <CoSelect v-model="sort" :options="sortOptions" />
            </label>
          </div>

          <div v-if="items.length > 0" class="content-filters__pager">
            <CoPagination
              compact
              :page="page"
              :total-pages="knownPages"
              :loading="loading"
              :labels="pagerLabels"
              @change="jumpPage"
            />
          </div>
        </aside>
      </template>
    </CoSplitLayout>
  </CoPage>
</template>

<style scoped>
/* 主栏只放结果；留白与滚动由 CoPage 承担。 */
.content-list {
  display: flex;
  flex-direction: column;
  min-width: 0;
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

/* ---------------------------------------------------------------- 右栏：搜索 / 筛选 / 分页 */
.content-filters {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  min-width: 0;
  padding: var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
}

.content-filters__search {
  position: relative;
  display: flex;
  align-items: center;
  height: var(--copper-control-h);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-bg);
  transition: border-color var(--copper-duration-fast) var(--copper-easing);
}

.content-filters__search:focus-within {
  border-color: var(--copper-accent);
}

.content-filters__search-icon {
  position: absolute;
  left: var(--copper-space-2);
  color: var(--copper-text-secondary);
  pointer-events: none;
}

.content-filters__search-input {
  width: 100%;
  height: 100%;
  padding: 0 var(--copper-space-2) 0 calc(var(--copper-space-2) + 22px);
  border: none;
  outline: none;
  background: transparent;
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
}

.content-filters__search-btn {
  height: var(--copper-control-h);
  border: none;
  border-radius: var(--copper-radius-md);
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition: opacity var(--copper-duration-fast) var(--copper-easing);
}

.content-filters__search-btn:hover {
  opacity: 0.9;
}

.content-filters__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-2);
  margin-top: var(--copper-space-2);
}

.content-filters__title {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  font-weight: 600;
  letter-spacing: 0.4px;
}

.content-filters__reset {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  transition: color var(--copper-duration-fast) var(--copper-easing);
}

.content-filters__reset:hover:not(:disabled) {
  color: var(--copper-accent);
}

.content-filters__reset:disabled {
  opacity: 0.4;
  cursor: default;
}

.content-filters__fields {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.content-filters__field {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--copper-space-2);
}

.content-filters__field-label {
  flex-shrink: 0;
  width: 52px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.content-filters__field :deep(.co-select) {
  flex: 1;
  min-width: 0;
}

.content-filters__field :deep(.co-select select) {
  width: 100%;
  height: var(--copper-control-h-sm);
  font-size: var(--copper-font-size-sm);
}

.content-filters__field-hint {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-left: 60px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.content-filters__field-hint--error {
  color: var(--copper-danger);
}

.content-filters__field-retry {
  border: none;
  background: transparent;
  color: var(--copper-accent);
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  text-decoration: underline;
}

.content-filters__pager {
  margin-top: var(--copper-space-1);
  padding-top: var(--copper-space-3);
  border-top: 1px solid var(--copper-border);
}

/* 翻页控件自带顶部间距；右栏已用分隔线断句，不再叠一层。 */
.content-filters__pager :deep(.co-pager) {
  padding-top: 0;
}

/* 折叠成单列后右栏横跨整行：筛选压成两列，避免把列表推到首屏之外。 */
@container (max-width: 760px) {
  .content-filters__fields {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--copper-space-2) var(--copper-space-3);
  }

  .content-filters__field-hint {
    margin-left: 0;
  }
}

/* ---------------------------------------------------------------- 长条卡片 */
.content-list__rows {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  flex: 1;
}

.content-list__state {
  flex: 1;
  /* 骨架不再给内容区固定高度，空 / 错状态至少占半屏，避免缩成顶部一行。 */
  min-height: 40vh;
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

.content-list__retry {
  display: inline-flex;
  align-items: center;
  gap: 6px;
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

.content-list__retry:hover {
  background: var(--copper-surface-2);
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
/* 翻页 UI 已抽到 components/ui/CoPagination.vue，本模块不再自带分页样式。 */

.spin {
  animation: spin 1.2s linear infinite;
}


@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
