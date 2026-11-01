<script setup lang="ts">
// 实例管理页副栏的分类过滤：纵向列表 + 计数，资源页与模组页共用。
//
// 过滤器放在副栏而不是列表上方，是因为稿子的右栏本来就是「搜索 + 一块面板」；
// 分类与计数属于辅助信息，放进去既填满了面板，也不挤占列表的横向空间。

import type { Component } from "vue";

defineProps<{
  /** 面板小标题；为空则不渲染标题行。 */
  title?: string;
  items: Array<{
    value: string;
    label: string;
    icon?: Component;
    /** 计数；undefined 表示不显示。 */
    count?: number;
  }>;
  active: string;
}>();

defineEmits<{
  "update:active": [value: string];
}>();
</script>

<template>
  <div class="filters">
    <span v-if="title" class="filters__title">{{ title }}</span>
    <button
      v-for="item in items"
      :key="item.value"
      type="button"
      :class="['filters__item', { 'filters__item--active': item.value === active }]"
      :aria-current="item.value === active ? 'true' : undefined"
      @click="$emit('update:active', item.value)"
    >
      <component :is="item.icon" v-if="item.icon" :size="14" :stroke-width="1.7" />
      <span class="filters__label">{{ item.label }}</span>
      <span v-if="item.count !== undefined" class="filters__count">{{ item.count }}</span>
    </button>
  </div>
</template>

<style scoped>
.filters {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.filters__title {
  padding: 0 var(--copper-space-2) var(--copper-space-1);
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.filters__item {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-2);
  border: none;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.filters__item:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.filters__item--active {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
  color: var(--copper-text);
}

.filters__label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.filters__count {
  flex-shrink: 0;
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
  font-variant-numeric: tabular-nums;
}
</style>
