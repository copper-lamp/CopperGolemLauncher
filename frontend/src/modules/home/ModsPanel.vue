<script setup lang="ts">
// 模组页右侧的操作面板。
//
// 顶部是安装快捷方式（拖入 ZIP / 点击打开模组目录），中间是整批动作
// （一键更新、下载模组），下面才是多选后的批量操作 —— 顺序按「先做全局的事，
// 再处理选中项」排，避免多选状态一出现就把导入入口顶下去。
//
// 拖拽用 Tauri 的 webview 拖放事件而不是 HTML5 `drop`：Tauri 默认接管原生拖放，
// `dataTransfer.files` 在 WebView2 里拿不到真实路径，只有 webview 事件能给出
// 磁盘路径（安装 ZIP 需要的是路径，不是 File 对象）。

import { computed, onMounted, onUnmounted, ref } from "vue";
import { Download, FileArchive, Loader2, RefreshCw, X } from "@lucide/vue";
import { getCurrentWebview } from "@tauri-apps/api/webview";

import { useI18n } from "../../i18n";
import CoButton from "../../components/ui/CoButton.vue";

const props = defineProps<{
  selectedCount: number;
  /** 恰好选中一个时才允许编辑清单（编辑的是单选语义的目标）。 */
  canEdit: boolean;
  /** 选中项里可更新的数量。 */
  selectedUpdatable: number;
  /** 整个实例的可更新数量。 */
  updatableCount: number;
  importing: boolean;
  updating: boolean;
}>();

const emit = defineEmits<{
  openFolder: [];
  importZip: [path: string];
  importZipDialog: [];
  importDll: [];
  edit: [];
  updateAll: [];
  updateSelected: [];
  download: [];
  enable: [];
  disable: [];
  remove: [];
  clear: [];
  rejectDrop: [];
}>();

const { t } = useI18n();

const zone = ref<HTMLElement | null>(null);
const dragging = ref(false);

const busy = computed(() => props.importing || props.updating);

/** 指针是否落在虚线框内（拖放事件给的是物理像素）。 */
function insideZone(position?: { x: number; y: number }): boolean {
  const el = zone.value;
  if (!el || !position) return false;
  const rect = el.getBoundingClientRect();
  const ratio = window.devicePixelRatio || 1;
  const x = position.x / ratio;
  const y = position.y / ratio;
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

let disposeDrag: (() => void) | null = null;

onMounted(async () => {
  try {
    disposeDrag = await getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "enter" || payload.type === "over") {
        dragging.value = insideZone(payload.position);
        return;
      }
      if (payload.type === "drop") {
        const inside = insideZone(payload.position);
        dragging.value = false;
        if (!inside) return;
        const zip = payload.paths.find((path) => path.toLowerCase().endsWith(".zip"));
        if (zip) emit("importZip", zip);
        else emit("rejectDrop");
        return;
      }
      dragging.value = false;
    });
  } catch {
    // 非 Tauri 环境（纯浏览器调试）没有 webview 拖放：快捷方式退化为纯点击。
    disposeDrag = null;
  }
});

onUnmounted(() => {
  disposeDrag?.();
  disposeDrag = null;
});
</script>

<template>
  <div class="panel">
    <div
      ref="zone"
      :class="['panel__drop', { 'panel__drop--active': dragging, 'panel__drop--busy': importing }]"
      role="button"
      tabindex="0"
      @click="emit('openFolder')"
      @keydown.enter.prevent="emit('openFolder')"
    >
      <Loader2 v-if="importing" :size="20" class="panel__spin" :stroke-width="1.6" />
      <FileArchive v-else :size="20" :stroke-width="1.6" />
      <span class="panel__drop-title">
        {{ importing ? t("module.home.mods.importing") : t("module.home.mods.drop_title") }}
      </span>
      <span class="panel__drop-hint">{{ t("module.home.mods.drop_hint") }}</span>
    </div>

    <div class="panel__actions">
      <CoButton
        variant="secondary"
        :disabled="busy || updatableCount === 0"
        @click="emit('updateAll')"
      >
        <RefreshCw :size="15" :class="{ 'panel__spin': updating }" />
        <span>{{ t("module.home.mods.update_all") }}</span>
        <span v-if="updatableCount > 0" class="panel__count">{{ updatableCount }}</span>
      </CoButton>
      <CoButton variant="secondary" :disabled="busy" @click="emit('download')">
        <Download :size="15" />
        <span>{{ t("module.home.mods.download") }}</span>
      </CoButton>
      <div class="panel__minor">
        <button type="button" class="panel__minor-btn" :disabled="busy" @click="emit('importZipDialog')">
          {{ t("module.home.mods.import_zip") }}
        </button>
        <button type="button" class="panel__minor-btn" :disabled="busy" @click="emit('importDll')">
          {{ t("module.home.mods.import_dll") }}
        </button>
      </div>
    </div>

    <section v-if="selectedCount > 0" class="panel__selection">
      <header class="panel__selection-head">
        <span class="panel__selection-title">
          {{ t("module.home.mods.selected", { count: selectedCount }) }}
        </span>
        <button
          type="button"
          class="panel__clear"
          :title="t('module.home.mods.clear_selection')"
          @click="emit('clear')"
        >
          <X :size="14" />
        </button>
      </header>
      <div class="panel__selection-actions">
        <CoButton
          variant="secondary"
          size="sm"
          :disabled="busy || selectedUpdatable === 0"
          @click="emit('updateSelected')"
        >
          <RefreshCw :size="14" />
          <span>{{ t("module.home.mods.apply_update") }}</span>
        </CoButton>
        <CoButton variant="secondary" size="sm" :disabled="busy" @click="emit('enable')">
          {{ t("module.home.mods.enable") }}
        </CoButton>
        <CoButton variant="secondary" size="sm" :disabled="busy" @click="emit('disable')">
          {{ t("module.home.mods.disable") }}
        </CoButton>
        <CoButton variant="danger" size="sm" :disabled="busy" @click="emit('remove')">
          {{ t("module.home.mods.remove") }}
        </CoButton>
      </div>

      <button
        v-if="canEdit"
        type="button"
        class="panel__minor-btn"
        :disabled="busy"
        @click="emit('edit')"
      >
        {{ t("module.home.mods.edit") }}
      </button>
    </section>

    <p v-else class="panel__hint">{{ t("module.home.mods.loader_hint") }}</p>
  </div>
</template>

<style scoped>
.panel {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-4);
  padding: var(--copper-space-4);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow-subtle);
}

/* 虚线框：安装快捷方式。不描边实线，虚线本身就是"可以扔进来"的提示。 */
.panel__drop {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-1);
  padding: var(--copper-space-5) var(--copper-space-3);
  border: 1px dashed color-mix(in srgb, var(--copper-border) 90%, transparent);
  border-radius: var(--copper-radius-md);
  color: var(--copper-text-secondary);
  text-align: center;
  cursor: pointer;
  outline: none;
  transition:
    border-color var(--copper-duration-fast) var(--copper-easing),
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.panel__drop:hover {
  border-color: color-mix(in srgb, var(--copper-accent) 55%, var(--copper-border));
  color: var(--copper-text);
}

.panel__drop:focus-visible {
  border-color: color-mix(in srgb, var(--copper-accent) 70%, transparent);
}

.panel__drop--active {
  border-color: var(--copper-accent);
  background: color-mix(in srgb, var(--copper-accent) 10%, transparent);
  color: var(--copper-text);
}

.panel__drop--busy {
  cursor: progress;
}

.panel__drop-title {
  font-size: var(--copper-font-size-md);
  font-weight: 500;
}

.panel__drop-hint {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.panel__actions {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.panel__actions :deep(.co-btn) {
  justify-content: flex-start;
}

.panel__count {
  margin-left: auto;
  padding: 0 6px;
  border-radius: var(--copper-radius-full);
  background: color-mix(in srgb, var(--copper-accent) 16%, transparent);
  color: color-mix(in srgb, var(--copper-accent) 85%, var(--copper-text));
  font-size: var(--copper-font-size-xs);
  font-variant-numeric: tabular-nums;
}

.panel__minor {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  padding: 0 var(--copper-space-1);
}

.panel__minor-btn {
  border: none;
  background: transparent;
  padding: 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
}

.panel__minor-btn:hover:not(:disabled) {
  color: var(--copper-text);
  text-decoration: underline;
  text-underline-offset: 3px;
}

.panel__minor-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.panel__selection {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding-top: var(--copper-space-2);
  animation: panel-in var(--copper-duration) var(--copper-easing);
}

.panel__selection-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-2);
}

.panel__selection-title {
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

.panel__clear {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-disabled);
  cursor: pointer;
}

.panel__clear:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.panel__selection-actions {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--copper-space-2);
}

.panel__selection-actions :deep(.co-btn) {
  width: 100%;
}

.panel__hint {
  margin-top: auto;
  font-size: var(--copper-font-size-xs);
  line-height: 1.6;
  color: var(--copper-text-disabled);
}

.panel__spin {
  animation: panel-spin 900ms linear infinite;
}

@keyframes panel-spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes panel-in {
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