<script setup lang="ts">
// 内容下载列表页：搜索 / 来源与类型过滤 / 分页 / 卡片徽标。

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
  ChevronLeft,
  ChevronRight,
  LoaderCircle,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import TipsRotator from "../../components/TipsRotator.vue";
import {
  contentDownloadList,
  type ContentItem,
  type ContentSource,
  type ContentType,
} from "./api";
import {
  contentType,
  error,
  hasMore,
  items,
  loading,
  PAGE_SIZE,
  page,
  scrollTop,
  searchInput,
  source,
  total,
} from "./listStore";

const { t } = useI18n();
const router = useRouter();

const MB_KEY = "module.content-download";

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

let loadSeq = 0;

/** 总页数（total 缺失或不确定时可退化为仅按 hasMore 累计）。 */
const totalPages = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)));

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

function typeLabel(ct: ContentType): string {
  return t(`${MB_KEY}.type${labelSuffix(ct)}`);
}

function labelSuffix(ct: ContentType): string {
  switch (ct) {
    case "behavior_pack":
      return "BehaviorPack";
    case "texture_pack":
      return "TexturePack";
    case "shader":
      return "Shader";
    default:
      return "LlMod";
  }
}

function sourceLabel(src: ContentSource): string {
  return src === "lip" ? t(`${MB_KEY}.sourceLip`) : t(`${MB_KEY}.sourceCurseforge`);
}

/** 适配游戏版本范围展示（最低～最高）。 */
function versionRange(item: ContentItem): string {
  const lo = item.minGameVersion;
  const hi = item.maxGameVersion;
  if (lo && hi && lo !== hi) return `${lo}–${hi}`;
  return lo || hi || t(`${MB_KEY}.unknownVersion`);
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

// 来源/类型变化即回到第一页并重新加载；返回列表时保留此前的数据与页码。
watch([source, contentType], () => {
  page.value = 0;
  void fetchPage();
});

const scrollerEl = ref<HTMLElement | null>(null);

onMounted(() => {
  // 从详情页返回时数据已在 store 中，无需重新拉取；仅恢复滚动位置。
  if (items.value.length === 0) {
    void fetchPage();
  } else {
    requestAnimationFrame(() => {
      if (scrollerEl.value) scrollerEl.value.scrollTop = scrollTop.value;
    });
  }
});
</script>

<template>
  <div ref="scrollerEl" class="content-list">
    <!-- 刷新按钮注入全局标题栏操作区 -->
    <Teleport to="#copper-titlebar-actions">
      <button
        class="content-list__refresh"
        title="t(`${MB_KEY}.refresh`)"
        :disabled="loading"
        @click="search"
      >
        <RefreshCw :size="15" :class="{ spin: loading }" />
      </button>
    </Teleport>

    <div class="content-list__toolbar">
        <div class="content-list__search">
          <Search :size="15" class="content-list__search-icon" />
          <input
            v-model="searchInput"
            class="content-list__search-input"
            :placeholder="t(`${MB_KEY}.searchPlaceholder`)"
            @keyup.enter="search"
          />
          <button class="content-list__search-btn" @click="search">
            {{ t(`${MB_KEY}.search`) }}
          </button>
        </div>

        <div class="content-list__filters">
          <div class="content-list__filter">
            <span class="content-list__filter-label">{{ t(`${MB_KEY}.source`) }}</span>
            <div class="content-list__segmented">
              <button
                v-for="opt in SOURCES"
                :key="opt.value"
                class="content-list__segment"
                :class="{ active: source === opt.value }"
                @click="source = opt.value"
              >
                {{ t(opt.label) }}
              </button>
            </div>
          </div>
          <div class="content-list__filter">
            <span class="content-list__filter-label">{{ t(`${MB_KEY}.type`) }}</span>
            <div class="content-list__segmented">
              <button
                v-for="opt in TYPES"
                :key="opt.value"
                class="content-list__segment"
                :class="{ active: contentType === opt.value }"
                @click="contentType = opt.value"
              >
                {{ t(opt.label) }}
              </button>
            </div>
          </div>
        </div>
      </div>

    <!-- 加载骨架 -->
    <div v-if="loading && items.length === 0" class="content-list__loading">
      <div class="content-list__grid">
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

    <!-- 卡片网格 -->
    <div v-else class="content-list__grid">
      <button
        v-for="item in items"
        :key="item.id"
        class="content-card"
        @click="openDetail(item)"
      >
        <div class="content-card__thumb">
          <img v-if="item.iconUrl" :src="item.iconUrl" :alt="item.name" loading="lazy" />
          <component :is="typeIcon(item.contentType)" v-else :size="26" class="content-card__thumb-fallback" />
        </div>
        <div class="content-card__body">
          <div class="content-card__name" :title="item.name">{{ item.name }}</div>
          <div class="content-card__author" v-if="item.author">
            {{ t(`${MB_KEY}.by`, { name: item.author }) }}
          </div>
          <div class="content-card__desc">{{ item.description }}</div>
          <div class="content-card__badges">
            <span class="content-card__badge content-card__badge--source">
              {{ sourceLabel(item.source) }}
            </span>
            <span class="content-card__badge">
              {{ typeLabel(item.contentType) }}
            </span>
            <span class="content-card__badge" v-if="item.latestVersion">
              {{ item.latestVersion }}
            </span>
            <span class="content-card__badge" v-if="versionRange(item) !== t(`${MB_KEY}.unknownVersion`)">
              {{ versionRange(item) }}
            </span>
          </div>
        </div>
        <div class="content-card__meta">
          <span class="content-card__downloads">
            <Download :size="13" />
            {{ item.downloadCount.toLocaleString() }}
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

.content-list__toolbar {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
}

.content-list__search {
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

.content-list__search-btn {
  flex-shrink: 0;
  height: calc(var(--copper-control-h) - 10px);
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-sm);
  background: var(--copper-accent);
  color: var(--copper-on-accent, #fff);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition: opacity var(--copper-duration-fast) var(--copper-easing);
}

.content-list__search-btn:hover {
  opacity: 0.9;
}

.content-list__filters {
  display: flex;
  flex-wrap: wrap;
  gap: var(--copper-space-4);
}

.content-list__filter {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.content-list__filter-label {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.content-list__segmented {
  display: inline-flex;
  padding: 2px;
  gap: 2px;
  border-radius: var(--copper-radius-sm);
  background: var(--copper-surface-2);
}

.content-list__segment {
  padding: 4px 10px;
  border: none;
  border-radius: var(--copper-radius-xs);
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.content-list__segment:hover {
  color: var(--copper-text);
}

.content-list__segment.active {
  background: var(--copper-surface);
  color: var(--copper-accent);
}

.content-list__grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: var(--copper-space-3);
  flex: 1;
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
  flex-direction: column;
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
  transform: translateY(-2px);
  border-color: color-mix(in srgb, var(--copper-accent) 45%, var(--copper-border));
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.12);
}

.content-card__thumb {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 120px;
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

.content-card__body {
  flex: 1;
  padding: var(--copper-space-2) 0 0;
  min-width: 0;
}

.content-card__name {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.content-card__author {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  margin-top: 2px;
}

.content-card__desc {
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  margin-top: var(--copper-space-1);
  min-height: calc(2em + var(--copper-space-1));
}

.content-card__badges {
  display: flex;
  flex-wrap: wrap;
  gap: var(--copper-space-1);
  margin-top: var(--copper-space-2);
}

.content-card__badge {
  padding: 2px 8px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.content-card__badge--source {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
  color: var(--copper-accent);
}

.content-card__meta {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  padding-top: var(--copper-space-1);
  border-top: 1px solid var(--copper-border);
  margin-top: var(--copper-space-2);
}

.content-card__downloads {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.content-card--skeleton .content-card__thumb {
  background: var(--copper-surface-3);
}

.content-card__skeleton-body {
  padding: var(--copper-space-2) 0 0;
}

.content-card__skeleton-line {
  height: 12px;
  margin-top: var(--copper-space-2);
  border-radius: var(--copper-radius-xs);
  background: var(--copper-surface-3);
}

.content-card__skeleton-line--short {
  width: 60%;
}

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
  gap: var(--copper-space-0-5);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  min-width: 130px;
  justify-content: center;
}

.content-list__page-no {
  font-weight: 600;
  color: var(--copper-text-primary);
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