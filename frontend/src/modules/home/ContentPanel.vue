<script setup lang="ts">
// 内容列表：只负责「列表 + 悬浮操作栏」。
//
// 搜索词与类型过滤由 `ContentTab` 承担（搜索框上提到面板工具栏），本组件接收已过滤
// 的条目；启用 / 停用 / 删除在此完成后经 `changed` 通知调用方重新拉取清单。

import { computed, ref, watch } from "vue";
import { Package, Trash2, ToggleLeft, ToggleRight } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoButton from "../../components/ui/CoButton.vue";
import {
  homeContentRemove,
  homeContentSetEnabled,
  type ContentItem,
  type ContentKind,
} from "../../api/home";

const props = defineProps<{
  /** 已完成搜索与类型过滤的条目。 */
  items: ContentItem[];
  versionName: string;
  loading: boolean;
  /** 内容目录不可用（未安装 / 未启动过游戏）。 */
  unavailable: boolean;
  /** 列表为空时展示的文案（区分「无内容」与「无匹配」）。 */
  emptyText: string;
}>();

const emit = defineEmits<{ changed: [] }>();

const { t } = useI18n();

const selectedId = ref<string | null>(null);

const selected = computed(
  () => props.items.find((i) => i.id === selectedId.value) ?? null,
);

// 清单变化后选中项可能已不存在，及时清空，避免操作栏停留在失效条目上。
watch(
  () => props.items,
  () => {
    if (selectedId.value && !props.items.some((i) => i.id === selectedId.value)) {
      selectedId.value = null;
    }
  },
);

async function toggleEnabled() {
  const item = selected.value;
  if (!item) return;
  try {
    await homeContentSetEnabled(props.versionName, item.id, !item.enabled);
    emit("changed");
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
    emit("changed");
  } catch (e) {
    showToast(String(e), "error");
  }
}

function kindLabel(kind: ContentKind): string {
  return t(`module.home.content.types.${kind}`);
}
</script>

<template>
  <div class="content-list">
    <div v-if="loading" class="content-list__state">
      {{ t("common.loading") }}
    </div>
    <div v-else-if="unavailable" class="content-list__state">
      {{ t("module.home.content.load_failed") }}
    </div>
    <div v-else-if="items.length === 0" class="content-list__state">
      <Package :size="22" :stroke-width="1.5" />
      <span>{{ emptyText }}</span>
    </div>

    <ul v-else class="content-list__items">
      <li
        v-for="item in items"
        :key="item.id"
        :class="['content-list__item', { 'content-list__item--selected': selectedId === item.id }]"
        @click="selectedId = selectedId === item.id ? null : item.id"
      >
        <div class="content-list__main">
          <span class="content-list__name">{{ item.name }}</span>
          <span class="content-list__kind">{{ kindLabel(item.kind) }}</span>
        </div>
        <span
          :class="[
            'content-list__state-tag',
            item.enabled ? 'content-list__state-tag--on' : 'content-list__state-tag--off',
          ]"
        >
          {{ item.enabled ? t("module.home.content.enabled") : t("module.home.content.disabled") }}
        </span>
      </li>
    </ul>

    <!-- 悬浮操作栏：选中条目后出现 -->
    <footer v-if="selected" class="content-list__bar">
      <span class="content-list__bar-name">{{ selected.name }}</span>
      <div class="content-list__bar-actions">
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
  </div>
</template>

<style scoped>
.content-list {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.content-list__items {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  list-style: none;
  padding: var(--copper-space-2);
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.content-list__item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-2) var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.content-list__item:hover {
  background: var(--copper-hover);
}

.content-list__item--selected {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
}

.content-list__main {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.content-list__name {
  font-size: var(--copper-font-size-md);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.content-list__kind {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.content-list__state-tag {
  flex-shrink: 0;
  font-size: var(--copper-font-size-xs);
  padding: 2px 8px;
  border-radius: var(--copper-radius-full);
}

.content-list__state-tag--on {
  color: color-mix(in srgb, var(--copper-accent) 80%, var(--copper-text));
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
}

.content-list__state-tag--off {
  color: var(--copper-text-disabled);
  background: var(--copper-surface-2);
}

.content-list__state {
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

.content-list__bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) var(--copper-space-4);
  border-top: 1px solid var(--copper-border);
  background: var(--copper-surface-2);
  animation: content-bar-in var(--copper-duration) var(--copper-easing);
}

.content-list__bar-name {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.content-list__bar-actions {
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