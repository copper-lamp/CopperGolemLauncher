<script setup lang="ts">
// 面板容器：默认模式的「启动区 / 列表区」共用同一个版式外壳。
//
// 只负责标题与网格边界，不关心内容；拖拽与顺序由页面决定，
// 这样同一个容器既能装实例磁贴，也能装世界 / 服务器卡片。

defineProps<{
  title: string;
  hint?: string;
}>();
</script>

<template>
  <section class="co-board">
    <header class="co-board__head">
      <h2 class="co-board__title">{{ title }}</h2>
      <span v-if="hint" class="co-board__hint">{{ hint }}</span>
      <slot name="actions" />
    </header>
    <div class="co-board__body">
      <slot />
    </div>
  </section>
</template>

<style scoped>
.co-board {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.co-board__head {
  display: flex;
  align-items: baseline;
  gap: var(--copper-space-2);
  margin-bottom: var(--copper-space-3);
}

.co-board__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
}

.co-board__hint {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
}

.co-board__body {
  min-width: 0;
}
</style>
