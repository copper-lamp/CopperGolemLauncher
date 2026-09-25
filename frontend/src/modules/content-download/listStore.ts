// 内容下载列表页的模块级浏览状态。
//
// 组件被卸载（进入详情页 `/content/:id`）再返回时，Vue 会重建 ListView，
// 若状态放在组件内部会全部重置。这里把列表状态提升到模块作用域，使
// 详情返回后仍保留来源 / 类型 / 版本 / 排序 / 搜索 / 页码 / 数据与滚动位置。

import { computed, ref } from "vue";

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

/** 总页数（total 缺失或不确定时可退化为仅按 hasMore 累计）。 */
export const totalPages = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)));

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
  scrollTop.value = 0;
}
