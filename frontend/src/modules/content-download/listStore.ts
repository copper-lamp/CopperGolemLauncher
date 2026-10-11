// 内容下载列表页的模块级浏览状态。
//
// 组件被卸载（进入详情页 `/content/:id`）再返回时，Vue 会重建 ListView，
// 若状态放在组件内部会全部重置。这里把列表状态提升到模块作用域，使
// 详情返回后仍保留来源 / 类型 / 版本 / 排序 / 搜索 / 页码 / 数据与滚动位置。

import { ref } from "vue";

import type { ContentItem, ContentSort, ContentSource, ContentType } from "./api";

/** 每页条数（与后端 PAGE_SIZE 一致）。 */
export const PAGE_SIZE = 40;

export const searchInput = ref("");
export const source = ref<ContentSource | "">("");
export const contentType = ref<ContentType | "">("");
/** 游戏版本过滤；空串 = 不限版本。 */
export const gameVersion = ref<string>("");
/** 排序方式；与后端 `SORT_*` 对应，默认下载量降序。 */
export const sort = ref<ContentSort>("downloads_desc");
export const page = ref(0);

export const items = ref<ContentItem[]>([]);
export const hasMore = ref(false);
/**
 * 内核返回的条目总数。**不可作为分页依据**（见 `knownPages`），只保留原样回填。
 */
export const total = ref(0);
export const loading = ref(false);
export const error = ref<string | null>(null);

/** 可选游戏版本列表（远端拉取后缓存，避免每次打开筛选都请求）。 */
export const versions = ref<string[]>([]);
export const versionsLoading = ref(false);
export const versionsError = ref<string | null>(null);
let versionsLoaded = false;

/** 列表容器上次的滚动位置（离开页面前记录，返回后恢复）。 */
export const scrollTop = ref(0);

/**
 * **已探索到的页数**，分页 UI 的唯一依据。
 *
 * 为什么不能用 `total` 算总页数 —— 内核三条列表路径的 `total` 语义各不相同：
 * | 路径 | `total` 的值 | 可用？ |
 * |---|---|---|
 * | `curseforge::list` | 接口回的 `total_count`，真总数 | 可用 |
 * | `lip::list` | **恒为 0**（lipr 索引不回总数） | 不可用 |
 * | `mixed_list`（默认「全部来源」走这条） | 每次翻页 `+= per_page`，是**累加值不是总数** | 不可用 |
 *
 * 默认路径恰恰是 `mixed_list`，按 `ceil(total / PAGE_SIZE)` 算出来恒为 1，
 * 分页条会整个消失（这正是回归里踩到的坑）。
 *
 * 抓取型接口不提供总数，那「总共有多少页」在**用户没翻到那里之前就是未知的**。
 * 与其编一个假数字，不如如实呈现「已探索到的页数」：
 * - `hasMore` 为真 → 至少还有下一页，页数取 `当前页 + 2`；
 * - `hasMore` 为假 → 当前页就是末页，页数取 `当前页 + 1`。
 * 于是末页时数字必然准确，往前翻时数字单调增长，且「下一页」永远可用。
 * 这个值只在本次浏览会话内有效：回到第 0 页（搜索 / 重置筛选）即归零重来。
 */
export const knownPages = ref(1);

/**
 * 拉取一页后推进已探索页数。
 *
 * 回到第 0 页（首次加载、搜索、重置筛选）时先归零：此时结果集已经换过，
 * 之前探索出来的页数对不上，留着会让页码序列出现点不存在的末页。
 * 其它情况只增不减 —— 翻回上一页时不能让它掉回去（页码序列会缩水）。
 */
export function advanceKnownPages(currentPage: number, more: boolean): void {
  const reached = currentPage + 1;
  if (currentPage === 0) knownPages.value = 1;
  knownPages.value = Math.max(knownPages.value, reached + (more ? 1 : 0));
}

/** 是否已成功拉取过版本列表（失败时允许重试）。 */
export function hasLoadedVersions(): boolean {
  return versionsLoaded;
}

/** 记录版本列表拉取成功。 */
export function markVersionsLoaded(): void {
  versionsLoaded = true;
}

/** 恢复视图状态：重置翻页与数据。 */
export function resetList() {
  page.value = 0;
  items.value = [];
  hasMore.value = false;
  total.value = 0;
  knownPages.value = 1;
  scrollTop.value = 0;
}
