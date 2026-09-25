<script setup lang="ts">
// 版本设置页：编排层。整体为三个部分 —— 左标签页、主面板、顶部标签页。
//
// - 左标签页：纵向 `CoTabs` 版本 rail，落在页面底色上，贴着主面板左边，
//   选中项与主面板咬合成一体（接缝处是向外张开的倒角）；
// - 顶部标签页：横向 `CoTabs`（基本设置 / 内容管理 / 模组管理），在**主面板之上**、
//   与主面板分离，整体靠右，选中项向下与主面板咬合；
// - 主面板：卡片容器，顶部通栏是搜索框（仅内容 / 模组两个分区显示），下方是分区内容。
//
// 版面用 2×2 网格约束：顶部标签页独占第 1 行第 2 列（因此它不占左标签页的宽度），
// 左标签页与主面板同处第 2 行 —— 二者的顶边自然对齐，左标签页无需手工让位。
//
// 标题栏的「开始 / 版本设置」面包屑来自路由 meta `breadcrumb`（见 register.ts），
// 因此本页不再自带返回箭头与页面标题。
//
// 分区内容拆在 `VersionBasicTab` / `ContentTab` / `ModTab`；搜索词虽然由主面板顶部
// 通栏统一承载（位置与样式共用），但两个分区各持一份状态，互不影响。

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { Gamepad2, Search } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoTabs from "../../components/ui/CoTabs.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import ContentTab from "./ContentTab.vue";
import ModTab from "./ModTab.vue";
import VersionBasicTab from "./VersionBasicTab.vue";
import IconEditDialog from "./IconEditDialog.vue";
import {
  homeVersionDelete,
  homeVersionGet,
  homeVersionsList,
  type VersionView,
} from "../../api/home";
import { onVersionInstalled, onVersionRemoved, onVersionsChanged } from "../../events";

type SectionTab = "basic" | "content" | "mods";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();

const versions = ref<VersionView[]>([]);
const current = ref<VersionView | null>(null);
const loading = ref(true);
const activeTab = ref<SectionTab>("basic");
const contentSearch = ref("");
const modsSearch = ref("");
const iconDialogOpen = ref(false);

/** 版本 rail 标签项（内容在 `#item` 插槽里按名字回查）。 */
const railItems = computed(() =>
  versions.value.map((v) => ({ value: v.name, label: v.name })),
);

const versionByName = computed(
  () => new Map(versions.value.map((v) => [v.name, v])),
);

/** 当前版本名（写入即同步路由 `?name=`，路由是唯一事实源）。 */
const currentName = computed({
  get: () => current.value?.name ?? "",
  set: (name: string) => pickVersion(name),
});

const sectionTabs = computed(() => [
  { value: "basic", label: t("module.home.tab.basic") },
  { value: "content", label: t("module.home.tab.content") },
  { value: "mods", label: t("module.home.tab.mods") },
]);

/** 面板工具栏搜索框：位置共用，值按分区独立。 */
const activeSearch = computed({
  get: () => (activeTab.value === "content" ? contentSearch.value : modsSearch.value),
  set: (value: string) => {
    if (activeTab.value === "content") contentSearch.value = value;
    else modsSearch.value = value;
  },
});

const searchPlaceholder = computed(() =>
  activeTab.value === "content"
    ? t("module.home.content.search_placeholder")
    : t("module.home.mods.search_placeholder"),
);

watch(
  () => route.query.name,
  (name) => {
    const target = name && typeof name === "string" ? name : versions.value[0]?.name;
    if (target) void selectVersion(target);
  },
);

let dispose: Array<() => void> = [];

onMounted(async () => {
  await refresh();
  dispose = [
    await onVersionInstalled(() => void refresh()),
    await onVersionRemoved(() => void refresh()),
    await onVersionsChanged(() => void refresh()),
  ];
});

onUnmounted(() => {
  dispose.forEach((u) => u());
  dispose = [];
});

async function refresh() {
  try {
    versions.value = await homeVersionsList();
    const queryName = route.query.name;
    const target =
      (typeof queryName === "string" && queryName) || versions.value[0]?.name || "";
    if (target) {
      await selectVersion(target);
    } else {
      current.value = null;
    }
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    loading.value = false;
  }
}

async function selectVersion(name: string) {
  try {
    current.value = await homeVersionGet(name);
  } catch (e) {
    showToast(String(e), "error");
  }
}

function pickVersion(name: string) {
  if (name === current.value?.name) return;
  void router.replace({ path: "/version-settings", query: { name } });
}

/** 基本设置分区写入成功：同步当前版本与清单里的同名项。 */
function onUpdated(updated: VersionView) {
  const previous = current.value?.name;
  current.value = updated;
  const index = versions.value.findIndex((v) => v.name === (previous ?? updated.name));
  if (index >= 0) versions.value[index] = updated;
  else void refresh();
  if (previous && previous !== updated.name) {
    void router.replace({ path: "/version-settings", query: { name: updated.name } });
  }
}

async function deleteVersion() {
  const target = current.value;
  if (!target) return;
  if (!window.confirm(t("module.home.delete_confirm", { name: target.name }))) return;
  try {
    await homeVersionDelete(target.name);
    showToast(t("module.home.toast.deleted"), "success");
    const remaining = versions.value.filter((v) => v.name !== target.name);
    if (remaining.length === 0) {
      void router.push("/");
      return;
    }
    await refresh();
    void router.replace({
      path: "/version-settings",
      query: { name: remaining[0].name },
    });
  } catch (e) {
    showToast(String(e), "error");
  }
}
</script>

<template>
  <div class="version-settings">
    <!-- 顶部标签页：主面板之上、整体靠右；网格第 1 行第 2 列，不占左标签页的宽度 -->
    <nav class="version-settings__top-tabs">
      <CoTabs v-model="activeTab" :items="sectionTabs" />
    </nav>

    <!-- 左：版本 rail（无卡片，选中项直接与右侧面板咬合） -->
    <aside class="version-settings__rail">
      <div v-if="loading" class="version-settings__rail-state">
        {{ t("common.loading") }}
      </div>
      <div v-else-if="versions.length === 0" class="version-settings__rail-state">
        <Gamepad2 :size="26" :stroke-width="1.5" />
        <span>{{ t("module.home.empty") }}</span>
      </div>
      <CoTabs v-else v-model="currentName" direction="vertical" :items="railItems">
        <template #item="{ item }">
          <span class="version-settings__rail-icon">
            <img
              v-if="versionByName.get(item.value)?.logo_data_url"
              :src="versionByName.get(item.value)?.logo_data_url ?? ''"
              alt=""
            />
            <Gamepad2 v-else :size="18" :stroke-width="1.6" />
          </span>
          <span class="version-settings__rail-text">
            <span class="version-settings__rail-name">{{ item.label }}</span>
            <span class="version-settings__rail-meta">
              {{ versionByName.get(item.value)?.game_version }}
            </span>
          </span>
        </template>
      </CoTabs>
    </aside>

    <!-- 主面板：与左标签页同处网格第 2 行，顶边天然对齐 -->
    <section class="version-settings__panel">
      <div v-if="activeTab !== 'basic'" class="version-settings__panel-top">
        <div class="version-settings__search">
          <Search :size="14" class="version-settings__search-icon" />
          <CoTextField v-model="activeSearch" :placeholder="searchPlaceholder" />
        </div>
      </div>

      <div class="version-settings__panel-body">
        <template v-if="current">
          <VersionBasicTab
            v-if="activeTab === 'basic'"
            :version="current"
            @updated="onUpdated"
            @remove="deleteVersion"
            @edit-cover="iconDialogOpen = true"
          />
          <ContentTab
            v-else-if="activeTab === 'content'"
            :version-name="currentName"
            :search="contentSearch"
          />
          <ModTab v-else :version-name="currentName" :search="modsSearch" />
        </template>
        <div v-else class="version-settings__empty">
          <p>{{ t("module.home.empty") }}</p>
          <p class="version-settings__empty-hint">{{ t("module.home.install_hint") }}</p>
        </div>
      </div>
    </section>

    <IconEditDialog
      v-model:open="iconDialogOpen"
      :version-name="currentName"
      :current-logo="current?.logo_data_url ?? null"
      @saved="refresh"
    />
  </div>
</template>

<style scoped>
/* 三个部分的版面约束：第 1 行放顶部标签页（第 2 列），第 2 行放左标签页与主面板。
   两者同行即同高同顶，左标签页的接缝才能与主面板左边严丝合缝。 */
.version-settings {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  grid-template-rows: auto minmax(0, 1fr);
  height: 100%;
  padding: var(--copper-space-4);
  min-height: 0;
}

/* 顶部标签页：在主面板之上、整体靠右。
   `align-self: start` 让条带顶到网格行顶（默认拉伸会把它压扁 1px）；
   `margin-bottom: -1px` 让选中项的底边框正好压住主面板的上边框
   （配合 CoTabs 的 `border-bottom-color: surface` 抹掉接缝）。 */
.version-settings__top-tabs {
  grid-column: 2;
  grid-row: 1;
  position: relative;
  z-index: 1;
  align-self: start;
  justify-self: end;
  margin-bottom: -1px;
}

/* 版本 rail：无卡片、可滚动（滚动条隐藏以免占位破坏 1px 咬合对齐）。
   `margin-right: -1px` 让选中态的 1px 右边框正好压住面板的 1px 左边框。 */
.version-settings__rail {
  grid-column: 1;
  grid-row: 2;
  width: 236px;
  min-height: 0;
  overflow-y: auto;
  margin-right: -1px;
  position: relative;
  z-index: 1;
  scrollbar-width: none;
}

.version-settings__rail::-webkit-scrollbar {
  width: 0;
  height: 0;
}

.version-settings__rail-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-2);
  padding: var(--copper-space-4);
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
  text-align: center;
}

.version-settings__rail-icon {
  width: 40px;
  height: 40px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  overflow: hidden;
}

.version-settings__rail-icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.version-settings__rail-text {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.version-settings__rail-name {
  font-size: var(--copper-font-size-md);
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.version-settings__rail-meta {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 主面板：左上 / 左下被 rail 咬合，故左端取直角。 */
.version-settings__panel {
  grid-column: 2;
  grid-row: 2;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: 0 var(--copper-radius-lg) var(--copper-radius-lg) 0;
  overflow: hidden;
}

/* 主面板顶部通栏：搜索框所在行，仅内容 / 模组两个分区渲染。 */
.version-settings__panel-top {
  flex-shrink: 0;
  padding: var(--copper-space-4) var(--copper-space-4) 0;
}

.version-settings__search {
  position: relative;
}

.version-settings__search-icon {
  position: absolute;
  left: var(--copper-space-3);
  top: 50%;
  transform: translateY(-50%);
  color: var(--copper-text-disabled);
  pointer-events: none;
  z-index: 1;
}

.version-settings__search :deep(.co-text-field) {
  padding-left: calc(var(--copper-space-3) + 20px);
}

.version-settings__panel-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.version-settings__empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-1);
  color: var(--copper-text-secondary);
  padding: var(--copper-space-4);
  text-align: center;
}

.version-settings__empty-hint {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
}
</style>
