<script setup lang="ts">
// 实例 · 资源页（路由 `/version-settings/content`）：左侧按分类平铺，右侧是搜索 + 选中条目详情。
//
// 版面仍是 6:4：左主区是「分组标题 + 整行条目」，右副栏顶部是独立搜索框、下方是操作面板。
// 页面不出现「已启用 / 已停用」——隔离实例的加载目录就是全部内容，启用状态是历史概念；
// 所以这里只有浏览、看详情、打开文件夹、删除四种动作。
//
// 左侧不套卡片、不设页头：分组标题直接落在页面底色上，条目铺满整列宽度，
// 靠 surface-2 / surface-3 的色阶与 hover 区分层次（与入口页同一套「微色阶」语言）。

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { listen } from '@tauri-apps/api/event';
import { Box, FileBox, Globe, Image, PackageOpen } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { compatibilityReason, useCompatibilityText } from "../../services/compatibility-messages";
import ContentDetail from "./ContentDetail.vue";
import InstanceSearchBox from "./InstanceSearchBox.vue";
import { homeContentList, type ContentItem, type ContentKind } from "../../api/home";
import { useVersionInstance } from "./useVersionInstance";

const { t } = useI18n();
const { currentName, current } = useVersionInstance();
const compatText = useCompatibilityText();
const contentAllowed = computed(() => !!current.value && (!current.value.compatibility || current.value.compatibility.capabilities.content));
const blockedReason = computed(() => compatibilityReason(current.value?.compatibility?.content_disabled_reason ?? current.value?.compatibility?.disabled_reason, compatText) || compatText("sharedUnavailable"));

const search = ref("");
const items = ref<ContentItem[]>([]);
const loading = ref(false);
const unavailable = ref(false);
const selectedId = ref<string | null>(null);

/** 分类顺序固定：与后端枚举一致，避免分组随数据抖动重排。 */
const KINDS: ContentKind[] = ["resources", "behavior", "worlds", "templates", "screenshots"];

const KIND_META: Record<ContentKind, { icon: typeof Box; key: string }> = {
  resources: { icon: Box, key: "module.home.content.types.resources" },
  behavior: { icon: PackageOpen, key: "module.home.content.types.behavior" },
  worlds: { icon: Globe, key: "module.home.content.types.worlds" },
  templates: { icon: FileBox, key: "module.home.content.types.templates" },
  screenshots: { icon: Image, key: "module.home.content.types.screenshots" },
};

function iconOf(kind: ContentKind) {
  return KIND_META[kind].icon;
}

function kindLabel(kind: ContentKind): string {
  return t(KIND_META[kind].key);
}

const filtered = computed(() => {
  const query = search.value.trim().toLowerCase();
  if (!query) return items.value;
  return items.value.filter((item) => item.name.toLowerCase().includes(query));
});

/** 按分类分组；空分组不渲染，避免满屏 0 计数。 */
const groups = computed(() =>
  KINDS.map((kind) => ({
    kind,
    label: kindLabel(kind),
    icon: iconOf(kind),
    items: filtered.value.filter((item) => item.kind === kind),
  })).filter((group) => group.items.length > 0),
);

const selected = computed(
  () => items.value.find((item) => item.id === selectedId.value) ?? null,
);

const emptyText = computed(() =>
  search.value.trim()
    ? t("module.home.content.empty_search")
    : t("module.home.content.empty"),
);

let refreshToken = 0;
async function refresh() {
  const token = ++refreshToken;
  const name = currentName.value;
  if (!name || !contentAllowed.value) { items.value = []; selectedId.value = null; loading.value = false; return; }
  loading.value = true;
  unavailable.value = false;
  try {
    const listed = await homeContentList(name);
    if (token !== refreshToken || name !== currentName.value || !contentAllowed.value) return;
    items.value = listed;
  } catch {
    if (token !== refreshToken) return;
    // 内容目录不可用（未安装 / 从未启动过游戏）时展示空态说明，不伪造数据。
    unavailable.value = true;
    items.value = [];
  } finally {
    if (token !== refreshToken) return;
    loading.value = false;
    // 刷新后选中项可能已被删除。
    if (selectedId.value && !items.value.some((item) => item.id === selectedId.value)) {
      selectedId.value = null;
    }
  }
}

function select(item: ContentItem) {
  selectedId.value = item.id;
}

watch([currentName, contentAllowed], () => void refresh(), { immediate: true });
let disposed = false;
let offContentChanged: (() => void) | undefined;
onMounted(async () => {
  const off = await listen('home.content-changed', () => { void refresh(); });
  if (disposed) off(); else offContentChanged = off;
});
onUnmounted(() => { disposed = true; offContentChanged?.(); });
</script>

<template>
  <div class="content">
    <section class="content__main">
      <div class="content__body">
        <div v-if="!contentAllowed" class="content__state" role="status">{{ blockedReason }}</div>
        <div v-else-if="loading" class="content__state">{{ t("common.loading") }}</div>
        <div v-else-if="unavailable" class="content__state">
          {{ t("module.home.content.load_failed") }}
        </div>
        <div v-else-if="groups.length === 0" class="content__state">
          <Box :size="22" :stroke-width="1.5" />
          <span>{{ emptyText }}</span>
        </div>

        <template v-else>
          <section v-for="group in groups" :key="group.kind" class="group">
            <header class="group__head">
              <component :is="group.icon" :size="14" :stroke-width="1.75" class="group__icon" />
              <span class="group__label">{{ group.label }}</span>
              <span class="group__count">{{ group.items.length }}</span>
            </header>
            <div class="group__list">
              <button
                v-for="item in group.items"
                :key="item.id"
                type="button"
                :class="['item', { 'item--active': selectedId === item.id }]"
                :title="item.name"
                @click="select(item)"
              >
                <span class="item__icon">
                  <component :is="iconOf(item.kind)" :size="16" :stroke-width="1.6" />
                </span>
                <span class="item__name">{{ item.name }}</span>
              </button>
            </div>
          </section>
        </template>
      </div>
    </section>

    <aside class="content__aside">
      <div class="content__search">
        <InstanceSearchBox
          v-model="search"
          :placeholder="t('module.home.content.search_placeholder')"
        />
      </div>
      <ContentDetail
        :item="contentAllowed ? selected : null"
        :can-manage="contentAllowed"
        :version-name="currentName"
        @removed="refresh"
      />
    </aside>
  </div>
</template>

<style scoped>
.content {
  display: grid;
  grid-template-columns: minmax(0, 6fr) minmax(0, 4fr);
  gap: var(--copper-space-4);
  height: 100%;
  min-height: 0;
  padding: var(--copper-space-4);
}

/* 左：分组列表铺满整列，不套卡片。 */
.content__main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.content__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-5);
  padding: var(--copper-space-1) 0 var(--copper-space-4);
}

.content__state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-2);
  padding: var(--copper-space-6);
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
}

.group {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.group__head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  padding: 0 var(--copper-space-2) var(--copper-space-1);
  color: var(--copper-text-secondary);
}

.group__icon {
  color: var(--copper-text-disabled);
}

.group__label {
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
  letter-spacing: 0.02em;
}

.group__count {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
  font-variant-numeric: tabular-nums;
}

.group__list {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

/* 整行条目：铺满左右，图标 + 名字；不描边，靠底色区分 hover / 选中。 */
.item {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  width: 100%;
  min-width: 0;
  min-height: 44px;
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text);
  text-align: left;
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.item:hover {
  background: var(--copper-surface-2);
}

.item--active {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
}

.item--active:hover {
  background: color-mix(in srgb, var(--copper-accent) 18%, transparent);
}

.item__icon {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: var(--copper-radius-sm);
  background: color-mix(in srgb, var(--copper-text) 6%, transparent);
  color: var(--copper-text-secondary);
}

.item--active .item__icon {
  background: color-mix(in srgb, var(--copper-accent) 20%, transparent);
  color: color-mix(in srgb, var(--copper-accent) 80%, var(--copper-text));
}

.item__name {
  min-width: 0;
  font-size: var(--copper-font-size-md);
  line-height: 1.4;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 右：搜索独立在顶部，详情面板撑满剩余高度。 */
.content__aside {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  min-width: 0;
  min-height: 0;
}

.content__search {
  flex-shrink: 0;
}

@media (max-width: 900px) {
  .content {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr) auto;
  }
}
</style>
