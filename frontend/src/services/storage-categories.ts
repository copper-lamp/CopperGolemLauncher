// 存储用量分类的单一事实来源：扫描顺序、图表色号、图例分组都从这里取。
//
// 后端 storage_usage 的 CATEGORY_IDS 与这里的顺序一致；新增分类时两边都要加，
// 并且只追加 tone 8、9…，不要改动已有色号，否则用户已看惯的图例颜色会整体换掉。
export interface StorageCategoryMeta {
  id: string;
  /** 图表色号：对应主题令牌 --copper-chart-{tone}。 */
  tone: number;
  group: "cache" | "temp" | "logs";
}

export const STORAGE_CATEGORIES: StorageCategoryMeta[] = [
  { id: "translation", tone: 1, group: "cache" },
  { id: "metadata", tone: 3, group: "cache" },
  { id: "loaders", tone: 4, group: "cache" },
  { id: "registry", tone: 5, group: "cache" },
  { id: "installers", tone: 7, group: "cache" },
  { id: "temp", tone: 6, group: "temp" },
  { id: "logs", tone: 2, group: "logs" },
];

/** 图例分组：缓存是「删了还能再下」，临时文件与日志是「删了只是丢掉痕迹」。 */
export const STORAGE_GROUPS: { id: "cache" | "temp" | "logs"; ids: string[] }[] = [
  { id: "cache", ids: ["translation", "metadata", "loaders", "registry", "installers"] },
  { id: "temp", ids: ["temp"] },
  { id: "logs", ids: ["logs"] },
];

export function storageCategoryColor(tone: number): string {
  return `var(--copper-chart-${tone})`;
}

export function storageCategoryMeta(id: string): StorageCategoryMeta | undefined {
  return STORAGE_CATEGORIES.find((item) => item.id === id);
}
