<script setup lang="ts">
// 实例管理页的搜索框：稿子里搜索在右副栏顶部，带一个「搜索」小标题。
//
// 做成独立组件是因为资源页与模组页各持一份搜索词、但外观必须完全一致：
// 图标叠在输入框里、清除按钮只在有内容时出现。两个页面各抄一份 CSS，
// 迟早会出现一边有清除按钮、一边没有的偏差。

import { Search, X } from "@lucide/vue";

import { useI18n } from "../../i18n";
import CoTextField from "../../components/ui/CoTextField.vue";

withDefaults(
  defineProps<{
    modelValue: string;
    placeholder: string;
    /** 紧凑模式：隐藏「搜索」小标题，用于放在页面标题栏里。 */
    compact?: boolean;
  }>(),
  { compact: false },
);

const emit = defineEmits<{
  "update:modelValue": [value: string];
}>();

const { t } = useI18n();
</script>

<template>
  <div :class="['search-box', { 'search-box--compact': compact }]">
    <span v-if="!compact" class="search-box__label">{{ t("common.search") }}</span>
    <div class="search-box__field">
      <Search :size="14" class="search-box__icon" />
      <CoTextField
        :model-value="modelValue"
        :placeholder="placeholder"
        @update:model-value="(value: string) => emit('update:modelValue', value)"
      />
      <button
        v-if="modelValue"
        type="button"
        class="search-box__clear"
        :title="t('common.cancel')"
        @click="emit('update:modelValue', '')"
      >
        <X :size="13" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.search-box {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.search-box--compact {
  gap: 0;
}

.search-box__label {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.search-box__field {
  position: relative;
}

.search-box__icon {
  position: absolute;
  left: var(--copper-space-3);
  top: 50%;
  transform: translateY(-50%);
  color: var(--copper-text-disabled);
  pointer-events: none;
  z-index: 1;
}

.search-box__field :deep(.co-text-field) {
  padding-left: calc(var(--copper-space-3) + 20px);
  padding-right: calc(var(--copper-space-3) + 18px);
}

.search-box__clear {
  position: absolute;
  right: var(--copper-space-2);
  top: 50%;
  transform: translateY(-50%);
  display: flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-disabled);
  cursor: pointer;
  z-index: 1;
}

.search-box__clear:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}
</style>
