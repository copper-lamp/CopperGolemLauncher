<script setup lang="ts">
// 内容管理面板：列出版本已加入资源（资源包 / 行为包 / 世界），
// 支持搜索与按类型过滤；点击条目呈选中态，底部悬浮操作栏执行 启用/停用 / 删除。

import { computed, onMounted, ref, watch } from "vue";
import { Search, Package, Trash2, ToggleLeft, ToggleRight } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoButton from "../../components/ui/CoButton.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import {
  homeContentList,
  homeContentRemove,
  homeContentSetEnabled,
  type ContentItem,
  type ContentKind,
} from "../../api/home";

const props = defineProps<{
  versionName: string;
}>();

const { t } = useI18n();

type KindFilter = "all" | ContentKind;

const items = ref<ContentItem[]>([]);
const search = ref("");
const kindFilter = ref<KindFilter>("all");
const selectedId = ref<string | null>(null);
const loading = ref(false);
const unavailable = ref(false);

const kindOptions = computed(() => [
  { value: "all", label: t("module.home.content.all") },
  { value: "resources", label: t("module.home.content.types.resources") },
  { value: "behavior", label: t("module.home.content.types.behavior") },
  { value: "worlds", label: t("module.home.content.types.worlds") },
]);

const filtered = computed(() => {
  const q = search.value.trim().toLowerCase();
  return items.value.filter((item) => {
    if (kindFilter.value !== "all" && item.kind !== kindFilter.value) return false;
    if (q && !item.name.toLowerCase().includes(q)) return false;
    return true;
  });
});

const selected = computed(
  () => items.value.find((i) => i.id === selectedId.value) ?? null,
);

watch(
  () => props.versionName,
  () => void refresh(),
);

onMounted(() => void refresh());

async function refresh() {
  loading.value = true;
  unavailable.value = false;
  selectedId.value = null;
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

async function toggleEnabled() {
  const item = selected.value;
  if (!item) return;
  try {
    await homeContentSetEnabled(props.versionName, item.id, !item.enabled);
    await refresh();
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function removeSelected() {
  const item = selected.value;
  if (!item) return;
  if (!window.confirm(t("module.home.content.remove_confirm", { name: item.name }))) return;
  try {
    await homeContentRemove(props.versionName, item.id);
    await refresh();
  } catch (e) {
    showToast(String(e), "error");
  }
}

function kindLabel(kind: ContentKind): string {
  return t(`module.home.content.types.${kind}`);
}
</script>

<template>
  <section class="content-panel">
    <header class="content-panel__header">
      <h3 class="content-panel__title">{{ t("module.home.content.title") }}</h3>
      <div class="content-panel__tools">
        <div class="content-panel__search">
          <Search :size="14" class="content-panel__search-icon" />
          <CoTextField
            v-model="search"
            :placeholder="t('home.content.search_placeholder')"
          />
        </div>
        <div class="content-panel__filters">
          <button
            v-for="opt in kindOptions"
            :key="opt.value"
            :class="[
              'content-panel__filter',
              { 'content-panel__filter--active': kindFilter === opt.value },
            ]"
            @click="kindFilter = opt.value as KindFilter"
          >
            {{ opt.label }}
          </button>
        </div>
      </div>
    </header>

    <div v-if="loading" class="content-panel__state">
      {{ t("common.loading") }}
    </div>
    <div v-else-if="unavailable" class="content-panel__state">
      {{ t("module.home.content.load_failed") }}
    </div>
    <div v-else-if="filtered.length === 0" class="content-panel__state">
      <Package :size="22" :stroke-width="1.5" />
      <span>{{ t("module.home.content.empty") }}</span>
    </div>

    <ul v-else class="content-panel__list">
      <li
        v-for="item in filtered"
        :key="item.id"
        :class="[
          'content-panel__item',
          { 'content-panel__item--selected': selectedId === item.id },
        ]"
        @click="selectedId = selectedId === item.id ? null : item.id"
      >
        <div class="content-panel__item-main">
          <span class="content-panel__item-name">{{ item.name }}</span>
          <span class="content-panel__item-kind">{{ kindLabel(item.kind) }}</span>
        </div>
        <span
          :class="[
            'content-panel__item-state',
            item.enabled
              ? 'content-panel__item-state--on'
              : 'content-panel__item-state--off',
          ]"
        >
          {{ item.enabled ? t("module.home.content.enabled") : t("module.home.content.disabled") }}
        </span>
      </li>
    </ul>

    <!-- 悬浮操作栏：选中条目后出现 -->
    <footer v-if="selected" class="content-panel__bar">
      <span class="content-panel__bar-name">{{ selected.name }}</span>
      <div class="content-panel__bar-actions">
        <CoButton variant="secondary" size="sm" @click="toggleEnabled">
          <ToggleRight v-if="!selected.enabled" :size="14" />
          <ToggleLeft v-else :size="14" />
          <span>
            {{
              selected.enabled
                ? t("module.home.content.disable")
                : t("module.home.content.enable")
            }}
          </span>
        </CoButton>
        <CoButton variant="danger" size="sm" @click="removeSelected">
          <Trash2 :size="14" />
          <span>{{ t("module.home.content.remove") }}</span>
        </CoButton>
      </div>
    </footer>
  </section>
</template>

<style scoped>
.content-panel {
  display: flex;
  flex-direction: column;
  min-height: 0;
  flex: 1;
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  overflow: hidden;
}

.content-panel__header {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4);
  border-bottom: 1px solid var(--copper-border);
}

.content-panel__title {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
}

.content-panel__tools {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  flex-wrap: wrap;
}

.content-panel__search {
  position: relative;
  flex: 1;
  min-width: 180px;
}

.content-panel__search-icon {
  position: absolute;
  left: var(--copper-space-3);
  top: 50%;
  transform: translateY(-50%);
  color: var(--copper-text-disabled);
  pointer-events: none;
  z-index: 1;
}

.content-panel__search .co-text-field {
  padding-left: calc(var(--copper-space-3) + 20px);
}

.content-panel__filters {
  display: inline-flex;
  gap: var(--copper-space-1);
  background: var(--copper-surface-2);
  padding: 2px;
  border-radius: var(--copper-radius-md);
}

.content-panel__filter {
  height: 26px;
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: calc(var(--copper-radius-md) - 2px);
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.content-panel__filter:hover {
  color: var(--copper-text);
}

.content-panel__filter--active {
  background: var(--copper-surface);
  color: var(--copper-text);
  font-weight: 500;
}

.content-panel__list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  list-style: none;
  padding: var(--copper-space-2);
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.content-panel__item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-2) var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.content-panel__item:hover {
  background: var(--copper-hover);
}

.content-panel__item--selected {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
}

.content-panel__item-main {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.content-panel__item-name {
  font-size: var(--copper-font-size-md);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.content-panel__item-kind {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.content-panel__item-state {
  flex-shrink: 0;
  font-size: var(--copper-font-size-xs);
  padding: 2px 8px;
  border-radius: var(--copper-radius-full);
}

.content-panel__item-state--on {
  color: color-mix(in srgb, var(--copper-accent) 80%, var(--copper-text));
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
}

.content-panel__item-state--off {
  color: var(--copper-text-disabled);
  background: var(--copper-surface-2);
}

.content-panel__state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-2);
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
  padding: var(--copper-space-4);
}

.content-panel__bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) var(--copper-space-4);
  border-top: 1px solid var(--copper-border);
  background: var(--copper-surface-2);
  animation: content-bar-in var(--copper-duration) var(--copper-easing);
}

.content-panel__bar-name {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.content-panel__bar-actions {
  display: flex;
  gap: var(--copper-space-2);
  flex-shrink: 0;
}

@keyframes content-bar-in {
  from {
    opacity: 0;
    transform: translateY(4px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}
</style>
