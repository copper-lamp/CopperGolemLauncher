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
/** 标签 / 分类过滤；空串 = 不限。 */
export const tag = ref<string>("");
/** 排序方式；与后端 `SORT_*` 对应，默认下载量降序。 */
export const sort = ref<ContentSort>("downloads_desc");
export const page = ref(0);

export const items = ref<ContentItem[]>([]);
/** Last successful validation survives detail/list remounts in this session. */
export const lastValidated = ref(0);
export function listNeedsValidation(now = Date.now()): boolean {
  return now - lastValidated.value >= 12 * 60 * 60 * 1000;
}
export const hasMore = ref(false);
/** 匹配总数（后端给的权威值）。 */
export const total = ref(0);
/** 总页数（后端算好的 1 基页数）；分页 UI 直接用，不再靠"已探索页数"猜。 */
export const totalPages = ref(1);
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
 * 总页数的来源：后端在每条列表路径上都算了真数。
 *
 * 上一版只能"已探索页数"猜测，因为 `total` 的语义在三条路径上不一致
 * （mixed 路径当时是累加假值）。现在三条路径统一返回 `totalPages`：
 * - `curseforge::list`：接口的 `total_count` / 每页条数；
 * - `lip::list` / `lla`：本地索引匹配数 / 每页条数；
 * - `mixed_list`：各类型页数的最大值（每页各类型只消耗自己的配额）；
 * - `search_list`：两源总数之和 / 每页条数。
 */

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
  lastValidated.value = 0;
  hasMore.value = false;
  total.value = 0;
  totalPages.value = 1;
  scrollTop.value = 0;
}
