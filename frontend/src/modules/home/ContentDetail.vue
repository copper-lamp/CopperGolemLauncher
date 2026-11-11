<script setup lang="ts">
// 资源详情面板：右侧副栏的操作面板。
//
// 只做「看」和两个动作：查看部分信息（预览图、类型、位置、大小、修改时间）、
// 打开所在文件夹、删除。路径完全来自后端枚举结果，前端不拼路径。
//
// 预览图由内核转成 data URL（未启用 assetProtocol），一次只取当前选中项，
// 切换选中项时用请求序号丢弃过期响应，避免快速点击时旧图覆盖新图。

import { computed, ref, watch } from "vue";
import {
  Box,
  FileBox,
  FolderOpen,
  Globe,
  Image,
  PackageOpen,
  SquareMousePointer,
  Trash2,
} from "@lucide/vue";

import { useI18n, useLocaleRef } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoButton from "../../components/ui/CoButton.vue";
import {
  homeContentOpenFolder,
  homeContentPreview,
  homeContentRemove,
  type ContentItem,
  type ContentKind,
} from "../../api/home";

const props = defineProps<{
  item: ContentItem | null;
  versionName: string;
  canManage?: boolean;
}>();

const emit = defineEmits<{ removed: [] }>();

const { t } = useI18n();
const locale = useLocaleRef();

const KIND_META: Record<ContentKind, { icon: typeof Box; key: string }> = {
  resources: { icon: Box, key: "module.home.content.types.resources" },
  behavior: { icon: PackageOpen, key: "module.home.content.types.behavior" },
  worlds: { icon: Globe, key: "module.home.content.types.worlds" },
  templates: { icon: FileBox, key: "module.home.content.types.templates" },
  screenshots: { icon: Image, key: "module.home.content.types.screenshots" },
};

const preview = ref<string | null>(null);
const previewLoading = ref(false);
const busy = ref(false);
let previewToken = 0;

const kindLabel = computed(() =>
  props.item ? t(KIND_META[props.item.kind].key) : "",
);
const kindIcon = computed(() =>
  props.item ? KIND_META[props.item.kind].icon : SquareMousePointer,
);

const sizeText = computed(() => {
  const bytes = props.item?.size;
  if (bytes === null || bytes === undefined) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`;
});

const modifiedText = computed(() => {
  const ms = props.item?.modified;
  if (ms === null || ms === undefined) return "—";
  try {
    return new Intl.DateTimeFormat(locale.value, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(new Date(ms));
  } catch {
    return new Date(ms).toLocaleString();
  }
});

async function loadPreview() {
  const item = props.item;
  const token = ++previewToken;
  preview.value = null;
  previewLoading.value = false;
  if (!item || !props.versionName) return;
  previewLoading.value = true;
  try {
    const url = await homeContentPreview(props.versionName, item.id);
    if (token !== previewToken) return;
    preview.value = url;
  } catch {
    // 预览是锦上添花：失败就回退到类型图标，不打断详情展示。
    if (token === previewToken) preview.value = null;
  } finally {
    if (token === previewToken) previewLoading.value = false;
  }
}

watch(() => props.item?.id, () => void loadPreview(), { immediate: true });

async function openFolder() {
  const item = props.item;
  if (!item) return;
  try {
    await homeContentOpenFolder(props.versionName, item.id);
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function remove() {
  const item = props.item;
  if (!item || busy.value || props.canManage === false) return;
  if (!window.confirm(t("module.home.content.remove_confirm", { name: item.name }))) return;
  busy.value = true;
  try {
    await homeContentRemove(props.versionName, item.id);
    emit("removed");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="detail">
    <div v-if="!item" class="detail__empty">
      <SquareMousePointer :size="26" :stroke-width="1.4" />
      <span>{{ t("module.home.content.detail.empty") }}</span>
    </div>

    <template v-else>
      <div class="detail__preview">
        <img v-if="preview" :src="preview" :alt="item.name" class="detail__image" />
        <component
          :is="kindIcon"
          v-else
          :size="38"
          :stroke-width="1.2"
          class="detail__placeholder"
        />
      </div>

      <div class="detail__scroll">
        <header class="detail__head">
          <h2 class="detail__name">{{ item.name }}</h2>
          <span class="detail__kind">{{ kindLabel }}</span>
        </header>

        <dl class="detail__info">
          <div class="detail__row">
            <dt>{{ t("module.home.content.detail.type") }}</dt>
            <dd>
              {{ kindLabel }} ·
              {{
                item.is_dir
                  ? t("module.home.content.detail.folder")
                  : t("module.home.content.detail.file")
              }}
            </dd>
          </div>
          <div class="detail__row">
            <dt>{{ t("module.home.content.detail.path") }}</dt>
            <dd class="detail__path">{{ item.path }}</dd>
          </div>
          <div class="detail__row">
            <dt>{{ t("module.home.content.detail.size") }}</dt>
            <dd>{{ item.is_dir ? "—" : sizeText }}</dd>
          </div>
          <div class="detail__row">
            <dt>{{ t("module.home.content.detail.modified") }}</dt>
            <dd>{{ modifiedText }}</dd>
          </div>
        </dl>

      </div>

      <footer class="detail__actions">
        <CoButton variant="secondary" :disabled="busy" @click="openFolder">
          <FolderOpen :size="15" />
          <span>{{ t("module.home.content.detail.open_folder") }}</span>
        </CoButton>
        <CoButton variant="danger" :disabled="busy || canManage === false" @click="remove">
          <Trash2 :size="15" />
          <span>{{ t("module.home.content.remove") }}</span>
        </CoButton>
      </footer>
    </template>
  </div>
</template>

<style scoped>
.detail {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--copper-surface);
  border-radius: var(--copper-radius-card);
  box-shadow: var(--copper-shadow-card);
  overflow: hidden;
}

.detail__empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-6);
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
  text-align: center;
}

/* 预览区：固定比例，图片 contain，不让不同尺寸的图标把面板撑变形。 */
.detail__preview {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  /* 16:9 且封顶 200px：长路径详情本来就高，预览不能再把操作按钮挤出视口。 */
  aspect-ratio: 16 / 9;
  max-height: 200px;
  margin: var(--copper-space-3) var(--copper-space-3) 0;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  overflow: hidden;
}

.detail__image {
  width: 100%;
  height: 100%;
  object-fit: contain;
  image-rendering: auto;
}

.detail__placeholder {
  color: color-mix(in srgb, var(--copper-text) 22%, transparent);
}

.detail__scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-4);
  padding: var(--copper-space-4);
}

.detail__head {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.detail__name {
  margin: 0;
  font-size: var(--copper-font-size-lg);
  font-weight: 650;
  line-height: 1.35;
  letter-spacing: -0.01em;
  overflow-wrap: anywhere;
}

.detail__kind {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.detail__info {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  margin: 0;
}

.detail__row {
  display: grid;
  grid-template-columns: 72px minmax(0, 1fr);
  gap: var(--copper-space-3);
  align-items: baseline;
}

.detail__row dt {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.detail__row dd {
  margin: 0;
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text);
  overflow-wrap: anywhere;
}

.detail__path {
  color: var(--copper-text-secondary) !important;
  font-variant-numeric: tabular-nums;
  line-height: 1.5;
}

/* 操作固定在面板底部：详情再长也不会把「打开文件夹 / 删除」滚出视口。 */
.detail__actions {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4) var(--copper-space-4);
  background: var(--copper-surface);
}

.detail__actions :deep(.co-button) {
  width: 100%;
  justify-content: center;
}
</style>
