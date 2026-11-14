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

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import TranslationAction from "../../components/TranslationAction.vue";
import CoSkeleton from "../../components/ui/CoSkeleton.vue";
import CoContentIcon from "../../components/ui/CoContentIcon.vue";
import { contentAuthorAvatar } from "./contentIcon";
import { useTranslationMessages } from "../../services/translation-messages";
import { useTranslation } from "../../composables/useTranslation";
import { useRouter } from "vue-router";
import {
  Search,
  ImageIcon,
  Layers,
  Sun,
  Puzzle,
  Download,
  User,
  RotateCcw,
  LoaderCircle,
  ArrowUp,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import CoBadge from "../../components/ui/CoBadge.vue";
import CoPagination from "../../components/ui/CoPagination.vue";
import CoPageScrubber from "../../components/ui/CoPageScrubber.vue";
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
  lastValidated,
  listNeedsValidation,
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
const translation = useTranslation();
const translationMessages = useTranslationMessages();
const { busy: translationBusy, translated: showTranslation, error: translationError } = translation;
function toggleTranslation() {
  if (translationBusy.value || showTranslation.value) { translation.reset(); return; }
  void translation.run(items.value.flatMap(item => [item.name, item.description ?? ""]));
}
function translatedField(index: number, field: 0 | 1, original: string) {
  return showTranslation.value ? translation.results.value[index * 2 + field] ?? original : original;
}
watch(items, translation.reset);
let refreshTimer: ReturnType<typeof setInterval> | undefined;

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
/** 拖动扫页的待定页码（1 基），只在拖动中存在：松手才真正翻页。 */
const pendingPage = ref<number | null>(null);
/** 左侧列表自己的滚动容器（整页不滚动，右栏才能顶满高度且不跟滚）。 */
const resultsRef = ref<HTMLElement | null>(null);
const tagOptions = ref<string[]>([]);
const showBackToTop = ref(false);

function scrollBody(): HTMLElement | null {
  return resultsRef.value ?? pageRef.value?.bodyEl ?? null;
}

function onResultsScroll(event: Event) {
  showBackToTop.value = (event.currentTarget as HTMLElement).scrollTop > 280;
}

function backToTop() {
  scrollBody()?.scrollTo({ top: 0, behavior: "smooth" });
}

function localizedTag(name: string): string {
  const key = MB_KEY + ".tags." + name.trim().toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_|_$/g, "");
  const translated = t(key);
  return translated === key ? name : translated;
}

/** 跳页滑轨的文案（与分页共用同一套 key，不新增词条）。 */
const scrubLabels = computed(() => ({
  jump: t(MB_KEY + ".pageJump"),
  goto: (n: number) => t(MB_KEY + ".pageGoto", { page: n }),
}));

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
  translation.reset();
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
    lastValidated.value = Date.now();
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

/** 扫页落定：清掉待定值，按 1 基页码真正翻页（同页不重复请求）。 */
function onScrubCommit(target: number) {
  pendingPage.value = null;
  jumpPage(target - 1);
}

function openDetail(item: ContentItem) {
  const scroller = scrollBody();
  if (scroller) scrollTop.value = scroller.scrollTop;
  void router.push("/content/" + encodeURIComponent(item.id));
}

async function loadVersions(force = false) {
  const ctype = contentType.value;
  if (!force && hasLoadedVersions(ctype) && versions.value.length > 0) return;
  if (versionsLoading.value) return;
  versionsLoading.value = true;
  versionsError.value = null;
  try {
    const list = await contentDownloadGameVersions(ctype || undefined);
    // 用户在请求期间切了类型，这份结果已经过期，丢弃而不是覆盖。
    if (contentType.value !== ctype) return;
    versions.value = list;
    markVersionsLoaded(ctype);
    versionsExpanded.value = false;
    // 版本表换了（MCBE ↔ Java），旧选中值在新表里不存在时清掉，避免筛出空列表。
    if (gameVersion.value && !list.includes(gameVersion.value)) gameVersion.value = "";
  } catch (e) {
    versionsError.value = String(e);
  } finally {
    versionsLoading.value = false;
  }
}

/**
 * 「游戏版本」组的展开状态。
 *
 * CurseForge 的 MCBE 版本表有上百个版本，全铺开会把后面的排序、标签挤出视野，
 * 所以默认只显示最新的若干个；展开按钮留在同一行，不额外占位。
 */
const VERSIONS_COLLAPSED = 12;
const versionsExpanded = ref(false);

/** 实际渲染的版本 chip：折叠时取最新的前若干个，但已选中的版本必须留在可见列表里。 */
const visibleVersions = computed(() => {
  if (versionsExpanded.value || versions.value.length <= VERSIONS_COLLAPSED) {
    return versions.value;
  }
  const head = versions.value.slice(0, VERSIONS_COLLAPSED);
  const selected = gameVersion.value;
  if (selected && !head.includes(selected)) head.push(selected);
  return head;
});

/** 折叠时被隐藏的版本数量（用于「更多版本」胶囊上的计数）。 */
const hiddenVersionCount = computed(() =>
  versionsExpanded.value ? 0 : Math.max(0, versions.value.length - VERSIONS_COLLAPSED),
);

/**
 * 版本号归到「大版本线」：`1.21.90` → `1.21`，`26.60` → `26`。
 *
 * 基岩版有两套编号：老版本是 `1.x.y`，26 起只有两段（`26.60`）。所以首位小于 2 时
 * 取前两段，否则只取首位，这样 `26.0.02` 与 `26.60` 会落在同一条 `26.x` 线上。
 */
function versionLine(version: string): string {
  const parts = version.split(/[.+-]/).filter(Boolean);
  if (parts.length <= 1) return parts[0] ?? version;
  return Number(parts[0]) >= 2 ? parts[0] : parts.slice(0, 2).join(".");
}

/**
 * 可见版本按大版本线分组，渲染时每条线前加一个 `1.21.x` 之类的行标题。
 *
 * 只对**已折叠后的可见列表**分组：展开后自然铺出全部版本线，折叠时通常只剩最新那条线，
 * 不会因为分组把十几条标题全铺进来。
 */
const visibleVersionGroups = computed(() => {
  const groups: { line: string; label: string; versions: string[] }[] = [];
  for (const version of visibleVersions.value) {
    const line = versionLine(version);
    const last = groups[groups.length - 1];
    if (last && last.line === line) last.versions.push(version);
    else groups.push({ line, label: `${line}.x`, versions: [version] });
  }
  return groups;
});

/** 标签表按来源变化：桌面端 lip 的标签与 CurseForge 的类别是两套词表。 */
async function loadTags() {
  // 记录发起时的筛选条件：标签词表同时取决于来源与类型（光影是 Realistic /
  // Fantasy / Vanilla，其它类型是 CurseForge 类别名）。切换类型会连发两次请求，
  // 先发的「全部类型」结果可能后到，直接覆盖会把已经切走的那套词表盖回来。
  const ctype = contentType.value;
  try {
    const options = await contentDownloadTags(
      source.value || undefined,
      ctype || undefined,
    );
    if (contentType.value !== ctype) return;
    tagOptions.value = options;
  } catch {
    if (contentType.value !== ctype) return;
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

// 光影的标签是另一套（Realistic / Fantasy / Vanilla），换类型必须重取词表。
watch(contentType, () => void loadTags());

// 内容类型换了就是换了一套游戏版本表（光影走 Java 版），旧版本号在新表里必然
// 不存在，先清掉选中值再重拉，否则会带着一个筛不出任何内容的版本号发请求。
watch(contentType, () => {
  gameVersion.value = "";
  versionsExpanded.value = false;
  void loadVersions();
});

onMounted(() => {
  // Check age while the page is mounted; new searches and pages keep their normal requests.
  refreshTimer = setInterval(() => {
    if (!loading.value && listNeedsValidation()) void fetchPage();
  }, 60 * 1000);
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
    if (listNeedsValidation()) void fetchPage();
    requestAnimationFrame(() => {
      const scroller = scrollBody();
      if (scroller) scroller.scrollTop = scrollTop.value;
    });
  }
});
onUnmounted(() => { ++loadSeq; loading.value = false; if (refreshTimer) clearInterval(refreshTimer); });
</script>
<template>
  <!-- 内容市场是宽屏工作台（列表 + 筛选栏），窗口最大化时左右铺满，不套 1240px 版心。 -->
  <CoPage ref="pageRef" :scroll="false" width="full">
    <Teleport defer to="#copper-titlebar-actions">
      <TranslationAction :busy="translationBusy" :translated="showTranslation" :error="translationError" :disabled="loading || !items.length" @toggle="toggleTranslation" />
    </Teleport>

    <div class="list-page">
      <div class="list-page__grid">
      <!-- 左：结果列表 -->
      <section ref="resultsRef" class="list-page__results" @scroll="onResultsScroll">
        <div v-if="loading && items.length === 0" class="list-page__state">
          <CoSkeleton variant="list" :rows="6" :label="translationMessages.contentLoading" />
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
            v-for="(item, itemIndex) in items"
            :key="item.id"
            class="content-card"
            @click="openDetail(item)"
          >
            <div class="content-card__thumb">
              <CoContentIcon v-if="item.contentType === 'll_mod'" :name="item.name" :src="item.iconUrl" :avatar="contentAuthorAvatar(item)" />
              <img v-else-if="item.iconUrl" :src="item.iconUrl" :alt="item.name" loading="lazy" />
              <component
                :is="typeIcon(item.contentType)"
                v-else
                :size="24"
                class="content-card__thumb-fallback"
              />
            </div>

            <div class="content-card__main">
              <div class="content-card__title-row">
                <span class="content-card__name" :title="item.name">{{ translatedField(itemIndex, 0, item.name) }}</span>
                <CoBadge
                  :tone="typeBadgeTone(item.contentType)"
                  :label="typeBadgeLabel(item.contentType, item.source)"
                />
                <CoBadge
                  :tone="sourceBadgeTone(item.source)"
                  :label="sourceBadgeLabel(item.source)"
                />
              </div>
              <div class="content-card__desc">{{ translatedField(itemIndex, 1, item.description) }}</div>
            </div>

            <!-- 右列元信息：作者 / 下载量 / 游戏版本。与图标、标题同为第一排，顶部对齐。 -->
            <div class="content-card__meta">
              <span class="content-card__meta-item" :title="item.author || ''">
                <User :size="15" />
                <span class="content-card__meta-text">
                  {{ item.author || t(MB_KEY + ".unknownVersion") }}
                </span>
              </span>
              <span class="content-card__meta-item" :title="t(MB_KEY + '.downloads')">
                <Download :size="15" />
                <span class="content-card__meta-text">{{ item.downloadCount.toLocaleString() }}</span>
              </span>
              <span class="content-card__meta-item" :title="t(MB_KEY + '.version')">
                <span class="content-card__meta-text">{{ versionRange(item) }}</span>
              </span>
            </div>

            <!-- 标签行整宽铺在图标下方那一排，从卡片左内边距起排。 -->
            <div class="content-card__tags">
              <span v-for="name in visibleTags(item)" :key="name" class="content-card__tag">
                {{ localizedTag(name) }}
              </span>
              <span v-if="(item.categories?.length ?? 0) > 3" class="content-card__tag">
                +{{ (item.categories?.length ?? 0) - 3 }}
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
              :preview="pendingPage"
              :labels="pagerLabels"
              @change="jumpPage"
            />

            <!-- 跳页滑轨：625 页时逐个点太慢。横拖扫页，拖出来的数字显示在
                 分页的当前页位上，松手才翻页（不跟着拖动疯狂请求）。 -->
            <CoPageScrubber
              :page="page"
              :total-pages="totalPages"
              :loading="loading"
              :labels="scrubLabels"
              @preview="pendingPage = $event"
              @commit="onScrubCommit"
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
                <template v-for="group in visibleVersionGroups" :key="group.line">
                  <span class="filter-group__line">{{ group.label }}</span>
                  <button
                    v-for="v in group.versions"
                    :key="v"
                    :class="['chip', { 'chip--active': gameVersion === v }]"
                    @click="gameVersion = v"
                  >
                    {{ v }}
                  </button>
                </template>
                <button
                  v-if="versions.length > VERSIONS_COLLAPSED"
                  class="chip chip--toggle"
                  @click="versionsExpanded = !versionsExpanded"
                >
                  {{
                    t(
                      MB_KEY +
                        (versionsExpanded ? ".versionCollapse" : ".versionExpand"),
                    )
                  }}
                  <span v-if="!versionsExpanded && hiddenVersionCount > 0" class="chip__count">
                    +{{ hiddenVersionCount }}
                  </span>
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
                  {{ localizedTag(name) }}
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
    </div>
    <button v-if="showBackToTop" class="list-page__back-to-top" type="button" :aria-label="t(MB_KEY + '.backToTop')" :title="t(MB_KEY + '.backToTop')" @click="backToTop">
      <ArrowUp :size="22" />
    </button>
  </CoPage>
</template>
<style scoped>
/* 容器查询的参照物必须是列布局的「祖先」：容器查询无法给容器自身套规则，
 * 所以 grid 单独放在 .list-page__grid 里，窄宽度才真能折成单列。 */
.list-page {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  container-type: inline-size;
}

.list-page__grid {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: minmax(0, 7fr) minmax(240px, 3fr);
  gap: var(--copper-space-4);
  align-items: stretch;
}

/* 左：列表自己的滚动容器。滚动条贴内容区右缘，不推动右栏。 */
.list-page__results {
  position: relative;
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
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: var(--copper-space-3);
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
  border-radius: var(--copper-radius-card);
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

/* 单卡结构（照 levimc / Modrinth 的项目卡）：
 * 第一排：左图标（顶部对齐标题）→ 中间「标题 + 徽标 / 两行描述」→ 右侧「作者 / 下载量 / 版本」（顶部对齐）。
 * 第二排：标签整宽铺开，正好落在图标下方那一行。
 * 卡片只用 surface 微色阶与页面底色分层，不加独立阴影、不描边。 */
.content-card {
  display: grid;
  grid-template-columns: 64px minmax(0, 1fr) minmax(96px, auto);
  grid-template-areas:
    "thumb main meta"
    "tags  tags tags";
  align-items: start;
  column-gap: 14px;
  row-gap: 10px;
  width: 100%;
  min-width: 0;
  padding: 14px 16px;
  text-align: left;
  background: var(--copper-surface);
  border: none;
  border-radius: var(--copper-radius-card);
  box-shadow: none;
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.content-card:hover {
  background: var(--copper-surface-2);
}

.content-card:hover .content-card__name {
  text-decoration: underline;
  text-underline-offset: 3px;
}

.content-card__thumb {
  grid-area: thumb;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 64px;
  height: 64px;
  border-radius: var(--copper-radius-lg);
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

/* 主信息区：标题行 + 两行描述，和图标顶边对齐。 */
.content-card__main {
  grid-area: main;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

/* 标题不占满整行：徽标紧跟在标题文字后面（参考图），标题过长时才让位给徽标。 */
.content-card__title-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.content-card__name {
  flex: 0 1 auto;
  min-width: 0;
  color: var(--copper-text);
  font-size: var(--copper-font-size-lg);
  font-weight: 700;
  line-height: 1.35;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.content-card__title-row :deep(.co-badge) {
  flex: 0 0 auto;
}

.content-card__desc {
  min-width: 0;
  min-height: 3em;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  line-height: 1.5;
}

/* 标签行占满整宽、单行排布：多了横向截断，剩余数量由「+n」胶囊交代。 */
.content-card__tags {
  grid-area: tags;
  display: flex;
  align-items: center;
  flex-wrap: nowrap;
  gap: 6px;
  height: 20px;
  min-width: 0;
  overflow: hidden;
}

.content-card__tag {
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  height: 20px;
  padding: 0 8px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  text-transform: capitalize;
}

/* 右列：三项元信息纵向排列、右对齐，相对卡片垂直居中（三行加高字号后靠上会显得悬空）。 */
.content-card__meta {
  grid-area: meta;
  justify-self: end;
  align-self: center;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  justify-content: center;
  gap: 8px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-md);
}

.content-card__meta-item {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  max-width: 100%;
}

.content-card__meta-item :deep(svg) {
  flex: 0 0 auto;
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
  box-shadow: var(--copper-shadow-card);
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
  display: flex;
  align-items: center;
  justify-content: center;
  height: 40px;
  padding: 0 var(--copper-space-4);
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
  border-radius: var(--copper-radius-card);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow-card);
}

.rail__pager :deep(.co-pager) {
  padding-top: 0;
  align-items: flex-start;
  gap: var(--copper-space-1);
}

.rail__pager :deep(.co-pager__summary) {
  display: none;
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
  align-items: center;
  gap: 4px;
}

/* 版本线的行标题：占满一整行强制换行，后面的 chip 自然落到下一行。 */
.filter-group__line {
  flex: 0 0 100%;
  margin-top: 2px;
  padding-left: 2px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  font-weight: 600;
  letter-spacing: 0.02em;
}

.filter-group__line:first-of-type {
  margin-top: 0;
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

/* 版本折叠开关：和普通 chip 同高，用强调色淡底区分「动作」与「选项」，
   避免被误认为某个游戏版本。 */
.chip--toggle {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  background: color-mix(in srgb, var(--copper-accent) 10%, transparent);
  color: color-mix(in srgb, var(--copper-accent) 82%, var(--copper-text));
}

.chip--toggle:hover {
  background: color-mix(in srgb, var(--copper-accent) 12%, transparent);
  color: color-mix(in srgb, var(--copper-accent) 92%, var(--copper-text));
}

.chip__count {
  font-size: 10px;
  font-weight: 600;
  opacity: 0.75;
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

/* 返回顶部：钉在页面左下角（侧边导航右侧的留白里），主题色实心圆，比普通图标按钮大一号。 */
.list-page__back-to-top {
  position: fixed;
  z-index: 20;
  left: calc(var(--copper-nav-w) + var(--copper-space-4));
  bottom: max(var(--copper-space-4), env(safe-area-inset-bottom));
  display: grid;
  place-items: center;
  width: 52px;
  height: 52px;
  border: none;
  border-radius: 50%;
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
  box-shadow: var(--copper-shadow-card);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.list-page__back-to-top:hover {
  background: color-mix(in srgb, var(--copper-accent) 88%, #000);
  transform: translateY(-2px);
}

@container (max-width: 860px) {
  .list-page__grid {
    grid-template-columns: minmax(0, 1fr);
    /* 单列后按内容自然高度排布，整页交给 CoPage 正文滚动。 */
    flex: 0 0 auto;
    height: auto;
  }

  .list-page__results {
    overflow: visible;
  }

  /* 窄宽度下先给内容、后给筛选：一进来就看到模组列表，而不是整屏操作栏。 */
  .rail {
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