<script setup lang="ts">
// 修改实例弹窗：把「版本设置」里那套可编辑项收进弹层，入口页保持一屏概览。
//
// 复用 `InstanceBasicForm`（封面 / 版本名 / 目录快捷方式 / 启动文件与注入）
// 而不是重写一份表单：这些设置项的校验与保存都在组件内部完成，抄一份等于把
// "改名失败要回滚草稿、还原启动文件后要重取状态"的边界逻辑也复制一遍。
// 模式分段与开关在入口页上，删除实例也归入口页，弹窗里不再重复。

import { X } from "@lucide/vue";

import { useI18n } from "../../i18n";
import CoButton from "../../components/ui/CoButton.vue";
import InstanceBasicForm from "./InstanceBasicForm.vue";
import type { VersionView } from "../../api/home";

defineProps<{
  open: boolean;
  version: VersionView | null;
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
  updated: [view: VersionView];
  "edit-cover": [];
}>();

const { t } = useI18n();

function close() {
  emit("update:open", false);
}
</script>

<template>
  <Teleport to="body">
    <div v-if="open && version" class="instance-dialog" role="dialog" aria-modal="true" @click.self="close">
      <div class="instance-dialog__card">
        <header class="instance-dialog__header">
          <h2 class="instance-dialog__title">{{ t("module.home.instance.edit.title") }}</h2>
          <button class="instance-dialog__close" :title="t('common.close')" @click="close">
            <X :size="16" />
          </button>
        </header>

        <div class="instance-dialog__body">
          <InstanceBasicForm
            :version="version"
            @updated="(view: VersionView) => emit('updated', view)"
            @edit-cover="emit('edit-cover')"
          />
        </div>

        <footer class="instance-dialog__footer">
          <CoButton variant="primary" @click="close">
            {{ t("module.home.instance.edit.done") }}
          </CoButton>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.instance-dialog {
  position: fixed;
  inset: 0;
  z-index: 100;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--copper-space-4);
  background: rgba(0, 0, 0, 0.5);
  animation: copper-fade var(--copper-duration) var(--copper-easing);
}

.instance-dialog__card {
  width: min(560px, calc(100vw - 48px));
  max-height: calc(100vh - 96px);
  display: flex;
  flex-direction: column;
  background: var(--copper-surface);
  border: 0; box-shadow: var(--copper-shadow-card);
  border-radius: var(--copper-radius-card);
  box-shadow: var(--copper-shadow-card);
  animation: copper-pop var(--copper-duration) var(--copper-easing);
  overflow: hidden;
}

.instance-dialog__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-shrink: 0;
  padding: var(--copper-space-4) var(--copper-space-4) 0;
}

.instance-dialog__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
}

.instance-dialog__close {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.instance-dialog__close:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

/* 正文：表单自己滚动，弹窗高度不随内容增长。 */
.instance-dialog__body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.instance-dialog__footer {
  flex-shrink: 0;
  display: flex;
  justify-content: flex-end;
  padding: var(--copper-space-3) var(--copper-space-4);
  background: var(--copper-surface-2);
  border-top: 1px solid var(--copper-border);
}

@keyframes copper-fade {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

@keyframes copper-pop {
  from {
    opacity: 0;
    transform: scale(0.96);
  }
  to {
    opacity: 1;
    transform: scale(1);
  }
}
</style>
