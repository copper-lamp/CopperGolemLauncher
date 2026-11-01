<script setup lang="ts">
// 实例 · 资源页（路由 `/version-settings/content`）：左侧列表，右侧搜索 + 类型筛选。
//
// 数据仍走 `home_content_list`（隔离实例读实例目录，非隔离版本经 AppX 包定位）。
// 列表与底部操作栏继续复用 `ContentPanel` —— 启用 / 停用 / 删除的交互
// （在加载目录与 `*_backup` 目录之间移动）不属于页面职责。
//
// 分类计数由清单自己算出：类型是后端给的枚举，前端不再多请求一次聚合。

import { computed, ref, watch } from "vue";
import { Box, FileBox, Globe, Layers, PackageOpen } from "@lucide/vue";

import { useI18n } from "../../i18n";
import ContentPanel from "./ContentPanel.vue";
import InstanceFilterList from "./InstanceFilterList.vue";
import InstanceManageShell from "./InstanceManageShell.vue";
import InstanceSearchBox from "./InstanceSearchBox.vue";
import { homeContentList, type ContentItem, type ContentKind } from "../../api/home";
import { useVersionInstance } from "./useVersionInstance";

const { t } = useI18n();
const { currentName } = useVersionInstance();

const category = ref("all");
const search = ref("");
const items = ref<ContentItem[]>([]);
const loading = ref(false);
const unavailable = ref(false);

/** 内容类型顺序固定：与后端枚举一致，避免分类栏随数据抖动重排。 */
const KINDS: ContentKind[] = ["resources", "behavior", "worlds", "templates"];

const KIND_META: Record<ContentKind, { icon: typeof Box; key: string }> = {
  resources: { icon: Box, key: "module.home.content.types.resources" },
  behavior: { icon: PackageOpen, key: "module.home.content.types.behavior" },
  worlds: { icon: Globe, key: "module.home.content.types.worlds" },
  templates: { icon: FileBox, key: "module.home.content.types.templates" },
};

function countOf(kind: ContentKind): number {
  return items.value.filter((item) => item.kind === kind).length;
}

const categories = computed(() => [
  {
    value: "all",
    label: t("module.home.content.all"),
    icon: Layers,
    count: items.value.length,
  },
  ...KINDS.map((kind) => ({
    value: kind,
    label: t(KIND_META[kind].key),
    icon: KIND_META[kind].icon,
    count: countOf(kind),
  })),
]);

const filtered = computed(() => {
  const query = search.value.trim().toLowerCase();
  return items.value.filter((item) => {
    if (category.value !== "all" && item.kind !== category.value) return false;
    if (query && !item.name.toLowerCase().includes(query)) return false;
    return true;
  });
});

const emptyText = computed(() =>
  search.value.trim() || category.value !== "all"
    ? t("module.home.content.empty_search")
    : t("module.home.content.empty"),
);

async function refresh() {
  const name = currentName.value;
  if (!name) return;
  loading.value = true;
  unavailable.value = false;
  try {
    items.value = await homeContentList(name);
  } catch {
    // 内容目录不可用（未安装 / 从未启动过游戏）时展示空态说明，不伪造数据。
    unavailable.value = true;
    items.value = [];
  } finally {
    loading.value = false;
  }
}

watch(currentName, () => void refresh(), { immediate: true });
</script>

<template>
  <InstanceManageShell
    :title="t('module.home.instance.page.content')"
    :count="items.length"
  >
    <ContentPanel
      :items="filtered"
      :version-name="currentName"
      :loading="loading"
      :unavailable="unavailable"
      :empty-text="emptyText"
      @changed="refresh"
    />

    <template #search>
      <InstanceSearchBox
        v-model="search"
        :placeholder="t('module.home.content.search_placeholder')"
      />
    </template>

    <template #aside>
      <InstanceFilterList
        v-model:active="category"
        :title="t('module.home.instance.filter')"
        :items="categories"
      />
    </template>
  </InstanceManageShell>
</template>
