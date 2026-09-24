<script setup lang="ts">
// 版本设置 · 内容管理分区。
//
// 承担内容条目清单的拉取、类型过滤与（面板工具栏提供的）搜索词过滤，
// 把过滤结果交给 `ContentPanel` 渲染列表与操作栏。
// 搜索词由面板工具栏统一承载，但状态按分区独立（本分区只读 `search` 不写回）。

import { computed, onMounted, ref, watch } from "vue";

import { useI18n } from "../../i18n";
import CoSegmented from "../../components/ui/CoSegmented.vue";
import ContentPanel from "./ContentPanel.vue";
import { homeContentList, type ContentItem, type ContentKind } from "../../api/home";

const props = defineProps<{
  versionName: string;
  /** 面板工具栏的搜索词（由 `VersionSettings` 传入）。 */
  search: string;
}>();

const { t } = useI18n();

type KindFilter = "all" | ContentKind;

const items = ref<ContentItem[]>([]);
const kindFilter = ref<KindFilter>("all");
const loading = ref(false);
const unavailable = ref(false);

const kindOptions = computed(() => [
  { value: "all", label: t("module.home.content.all") },
  { value: "resources", label: t("module.home.content.types.resources") },
  { value: "behavior", label: t("module.home.content.types.behavior") },
  { value: "worlds", label: t("module.home.content.types.worlds") },
]);

const filtered = computed(() => {
  const q = props.search.trim().toLowerCase();
  return items.value.filter((item) => {
    if (kindFilter.value !== "all" && item.kind !== kindFilter.value) return false;
    if (q && !item.name.toLowerCase().includes(q)) return false;
    return true;
  });
});

const emptyText = computed(() =>
  props.search.trim() || kindFilter.value !== "all"
    ? t("module.home.content.empty_search")
    : t("module.home.content.empty"),
);

watch(
  () => props.versionName,
  () => void refresh(),
);

onMounted(() => void refresh());

async function refresh() {
  loading.value = true;
  unavailable.value = false;
  try {
    items.value = await homeContentList(props.versionName);
  } catch {
    // 内容目录不可用（未安装 / 未启动过游戏）时展示空态说明。
    unavailable.value = true;
    items.value = [];
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <div class="content-tab">
    <div class="content-tab__filters">
      <CoSegmented v-model="kindFilter" :options="kindOptions" />
    </div>
    <ContentPanel
      :items="filtered"
      :version-name="versionName"
      :loading="loading"
      :unavailable="unavailable"
      :empty-text="emptyText"
      @changed="refresh"
    />
  </div>
</template>

<style scoped>
.content-tab {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.content-tab__filters {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4);
  border-bottom: 1px solid var(--copper-border);
}
</style>