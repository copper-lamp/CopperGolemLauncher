<script setup lang="ts">
// 设置分区卡片：分组标题 + 设置行列表。

import { useI18n } from "../../i18n";

const { t } = useI18n();

withDefaults(
  defineProps<{
    titleKey?: string;
  }>(),
  { titleKey: "" },
);
</script>

<template>
  <section class="setting-section">
    <h2 v-if="titleKey" class="setting-section__title">{{ t(titleKey) }}</h2>
    <div class="setting-section__card">
      <slot />
    </div>
  </section>
</template>

<style scoped>
.setting-section {
  margin-bottom: var(--copper-space-5);
}

.setting-section__title {
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
  color: var(--copper-text-secondary);
  margin-bottom: var(--copper-space-2);
  text-transform: uppercase;
  letter-spacing: 0.4px;
}

.setting-section__card {
  padding: var(--copper-space-1) var(--copper-space-4);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
}
</style>
