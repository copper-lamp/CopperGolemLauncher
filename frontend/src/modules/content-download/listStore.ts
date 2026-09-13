// 内容下载列表页的模块级浏览状态。
//
// 组件被卸载（进入详情页 `/content/:id`）再返回时，Vue 会重建 ListView，
// 若状态放在组件内部会全部重置。这里把列表状态提升到模块作用域，使
// 详情返回后仍保留来源 / 类型 / 搜索 / 页码 / 数据与滚动位置。

import { computed, ref } from "vue";

import type { ContentItem, ContentSource, ContentType } from "./api";

/** 每页条数（与后端 PAGE_SIZE 一致）。 */
export const PAGE_SIZE = 40;

export const searchInput = ref("");
export const source = ref<ContentSource | "">("");
export const contentType = ref<ContentType | "">("");
export const page = ref(0);

export const items = ref<ContentItem[]>([]);
export const hasMore = ref(false);
export const total = ref(0);
export const loading = ref(false);
export const error = ref<string | null>(null);

/** 列表容器上次的滚动位置（离开页面前记录，返回后恢复）。 */
export const scrollTop = ref(0);

/** 总页数（total 缺失或不确定时可退化为仅按 hasMore 累计）。 */
export const totalPages = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)));

/** 恢复视图状态：重置翻页与数据。 */
export function resetList() {
  page.value = 0;
  items.value = [];
  hasMore.value = false;
  total.value = 0;
  scrollTop.value = 0;
}