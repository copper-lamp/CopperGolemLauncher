<script setup lang="ts">
// 设置行：标签 + 可选提示 + 右侧控件插槽。

import { useI18n } from "../../i18n";

const { t } = useI18n();

withDefaults(
  defineProps<{
    labelKey: string;
    hintKey?: string;
  }>(),
  { hintKey: "" },
);
</script>

<template>
  <div class="setting-row">
    <div class="setting-row__text">
      <div class="setting-row__label">{{ t(labelKey) }}</div>
      <div v-if="hintKey" class="setting-row__hint">{{ t(hintKey) }}</div>
    </div>
    <div class="setting-row__control">
      <slot />
    </div>
  </div>
</template>

<style scoped>
.setting-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-4);
  min-height: 52px;
  padding: var(--copper-space-2) 0;
}

.setting-row__text {
  min-width: 0;
}

.setting-row__label {
  font-size: var(--copper-font-size-md);
}

.setting-row__hint {
  margin-top: 2px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.setting-row__control {
  flex-shrink: 0;
}
</style>
