<script setup lang="ts">
// 版本设置 · 基本设置分区。
//
// 封面（点击进入编辑弹窗）、版本名（失焦 / 回车提交重命名）、元信息（类型 · 游戏版本 ·
// 已注册），以及 版本文件夹 / 渲染龙 / 世界编辑器 三个设置行与「删除版本」。
//
// 重命名与设置写入在本组件内完成（便于失败时回滚输入框草稿），成功后经 `updated`
// 把新视图交回 `VersionSettings` 同步清单与路由；删除涉及路由跳转，由父级处理。

import { computed, ref, watch } from "vue";
import { FolderOpen, Gamepad2, Trash2 } from "@lucide/vue";
import { openPath } from "@tauri-apps/plugin-opener";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoButton from "../../components/ui/CoButton.vue";
import CoSwitch from "../../components/ui/CoSwitch.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { homeVersionRename, homeVersionSaveMeta, type VersionView } from "../../api/home";

const props = defineProps<{
  version: VersionView;
}>();

const emit = defineEmits<{
  /** 重命名 / 设置写入成功后回传新视图。 */
  updated: [view: VersionView];
  /** 请求删除当前版本（确认与路由跳转由父级处理）。 */
  remove: [];
  /** 请求打开封面编辑弹窗。 */
  "edit-cover": [];
}>();

const { t } = useI18n();

const nameDraft = ref(props.version.name);

watch(
  () => props.version.name,
  (name) => {
    nameDraft.value = name;
  },
);

const typeLabel = computed(() => {
  const key = `module.home.type.${props.version.version_type}`;
  const label = t(key);
  return label === key ? props.version.version_type : label;
});

/** 提交重命名（值未变时直接还原草稿）。 */
async function saveName() {
  const target = props.version;
  const next = nameDraft.value.trim();
  if (!next || next === target.name) {
    nameDraft.value = target.name;
    return;
  }
  try {
    const updated = await homeVersionRename(target.name, next);
    showToast(t("module.home.toast.renamed"), "success");
    emit("updated", updated);
  } catch (e) {
    showToast(String(e), "error");
    nameDraft.value = target.name;
  }
}

async function saveMeta(update: {
  enable_render_dragon?: boolean;
  enable_editor_mode?: boolean;
}) {
  try {
    const updated = await homeVersionSaveMeta(props.version.name, update);
    emit("updated", updated);
  } catch (e) {
    showToast(String(e), "error");
  }
}

function openFolder() {
  void openPath(props.version.folder);
}
</script>

<template>
  <div class="basic">
    <div class="basic__head">
      <button
        class="basic__cover"
        :title="t('module.home.basic.logo_hint')"
        @click="emit('edit-cover')"
      >
        <img v-if="version.logo_data_url" :src="version.logo_data_url" alt="" />
        <Gamepad2 v-else :size="34" :stroke-width="1.3" />
      </button>

      <div class="basic__ident">
        <span class="basic__label">{{ t("module.home.basic.name") }}</span>
        <CoTextField
          v-model="nameDraft"
          :placeholder="t('module.home.rename_placeholder')"
          @enter="saveName"
          @blur="saveName"
        />
        <p class="basic__meta">
          {{ typeLabel }} · {{ version.game_version }}
          <span v-if="version.registered" class="basic__registered">
            · {{ t("module.home.meta.registered") }}
          </span>
        </p>
      </div>
    </div>

    <ul class="basic__rows">
      <li class="basic__row">
        <span class="basic__row-title">{{ t("module.home.open_folder") }}</span>
        <button class="basic__path" :title="version.folder" @click="openFolder">
          <FolderOpen :size="14" />
          <span class="basic__path-text">{{ version.folder }}</span>
        </button>
      </li>
      <li class="basic__row">
        <span class="basic__row-title">{{ t("module.home.meta.render_dragon") }}</span>
        <CoSwitch
          :model-value="version.enable_render_dragon"
          @update:model-value="(v: boolean) => void saveMeta({ enable_render_dragon: v })"
        />
      </li>
      <li class="basic__row">
        <span class="basic__row-title">{{ t("module.home.meta.editor_mode") }}</span>
        <CoSwitch
          :model-value="version.enable_editor_mode"
          @update:model-value="(v: boolean) => void saveMeta({ enable_editor_mode: v })"
        />
      </li>
    </ul>

    <footer class="basic__footer">
      <CoButton variant="danger" size="sm" @click="emit('remove')">
        <Trash2 :size="14" />
        <span>{{ t("module.home.delete") }}</span>
      </CoButton>
    </footer>
  </div>
</template>

<style scoped>
.basic {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-4);
  padding: var(--copper-space-4);
  overflow-y: auto;
}

.basic__head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-4);
}

.basic__cover {
  width: 84px;
  height: 84px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  cursor: pointer;
  overflow: hidden;
  transition:
    border-color var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.basic__cover:hover {
  border-color: var(--copper-accent);
  transform: scale(1.02);
}

.basic__cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.basic__ident {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.basic__label {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.basic__meta {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.basic__registered {
  color: color-mix(in srgb, var(--copper-accent) 85%, var(--copper-text));
}

.basic__rows {
  list-style: none;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--copper-border);
}

.basic__row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) 0;
  border-bottom: 1px solid var(--copper-border);
}

.basic__row-title {
  font-size: var(--copper-font-size-md);
  flex-shrink: 0;
}

.basic__path {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-2);
  min-width: 0;
  max-width: 60%;
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.basic__path:hover {
  background: var(--copper-surface-3);
  color: var(--copper-text);
}

.basic__path-text {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.basic__footer {
  display: flex;
  justify-content: flex-end;
}
</style>