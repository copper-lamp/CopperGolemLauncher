<script setup lang="ts">
// 实例管理页骨架：左列表、右搜索栏（6:4），资源 / 模组 / 客户端三页共用。
//
// 版面直接照 V3 稿的「下载中心 / 模组管理 / 资源管理」：左侧是一整块列表卡片，
// 右侧 40% 是「搜索 + 一块面板」的副栏。此前把搜索放在列表上方、把分类做成左侧
// 竖排导航，与稿子不符，已推翻。
//
// 副栏面板在稿中是空的（职责未定，见 docs/CGL-UI-V3-设计稿评审.md 4.2），这里落成
// 「筛选与操作」：分类过滤、打开目录这类不该打断列表扫读的控件放进来，正好填满它，
// 也不改变稿子的信息结构。
//
// 只负责版面：列表、搜索、筛选项都由页面通过插槽提供，骨架不碰数据。

withDefaults(
  defineProps<{
    /** 左右列宽比例；默认 6:4，与设计稿一致。 */
    ratio?: string;
    /** 左列表卡片的标题；为空则不渲染标题栏。 */
    title?: string;
    /** 标题旁的计数徽标；undefined 表示不显示。 */
    count?: number;
  }>(),
  { ratio: "6fr 4fr", title: "", count: undefined },
);
</script>

<template>
  <div class="shell" :style="{ '--shell-ratio': ratio }">
    <section class="shell__main">
      <header v-if="title" class="shell__header">
        <span class="shell__title">{{ title }}</span>
        <span v-if="count !== undefined" class="shell__count">{{ count }}</span>
        <span class="shell__header-extra"><slot name="header" /></span>
      </header>
      <div class="shell__body">
        <slot />
      </div>
    </section>

    <aside class="shell__aside">
      <div v-if="$slots.search" class="shell__search">
        <slot name="search" />
      </div>
      <div class="shell__panel">
        <slot name="aside" />
      </div>
    </aside>
  </div>
</template>

<style scoped>
.shell {
  display: grid;
  /* 6:4：列表是主区，副栏只放搜索与筛选，不再抢宽度。 */
  grid-template-columns: minmax(0, 6fr) minmax(0, 4fr);
  gap: var(--copper-space-3);
  height: 100%;
  min-height: 0;
  padding: var(--copper-space-4);
}

/* 左：列表卡片。列表自己的滚动条贴在卡片内缘，不推动页面。 */
.shell__main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  overflow: hidden;
}

.shell__header {
  flex-shrink: 0;
  display: flex;
  align-items: baseline;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4);
  border-bottom: 1px solid var(--copper-border);
}

.shell__title {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
}

.shell__count {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
  font-variant-numeric: tabular-nums;
}

.shell__header-extra {
  margin-left: auto;
}

.shell__body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

/* 右：搜索 + 面板。面板撑满剩余高度，短窗口内自己滚动。 */
.shell__aside {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  min-width: 0;
  min-height: 0;
}

.shell__search {
  flex-shrink: 0;
}

.shell__panel {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
}

/* 窄窗口：副栏收一档，主栏保持可读宽度。 */
@media (max-width: 860px) {
  .shell {
    grid-template-columns: minmax(0, 6fr) minmax(0, 4fr);
  }
}
</style>
