<script setup lang="ts">
// 页面骨架：内容区宿主（V3 布局系统）。
//
// 它只负责三件事，且必须由它统一承担，页面不得各自实现：
// 1. 高度与滚动所有权：根节点撑满 `.app-content`，滚动只发生在 `.co-page__body`。
//    页面内的 sticky（分栏副栏、设置导航）都以这个滚动容器为参照；
// 2. 留白：统一取 `--copper-page-pad-*`，窄窗口与移动端自动收窄；
// 3. 宽度上限：内容最宽 `--copper-page-max` 并居中，窗口最大化时一行文字不会被拉长。
//
// 约定（见 docs/铜核心/界面布局.md）：
// - `#header` 不参与滚动，用于常驻的筛选条 / 页头；
// - `scroll=false` 用于自带滚动容器的页面（列表 / 详情迁移期），此时滚动交给页面内部；
// - `padded=false` 用于需要贴边的整幅布局，留白由页面自己按令牌补。
//
// 滚动为什么放在 `.co-page__body` 而不是根节点：根节点若滚动，标题栏下方会滚出一条
// 多余的滚动条，且 `position: sticky` 的参照系会变成根节点自身，分栏副栏会跟着整页跑。

withDefaults(
  defineProps<{
    /** 正文是否由本组件滚动；false 时滚动交给页面内部容器。 */
    scroll?: boolean;
    /** 是否施加统一留白。 */
    padded?: boolean;
    /** 是否限制内容最大宽度（`--copper-page-max`）。 */
    width?: "default" | "full";
  }>(),
  { scroll: true, padded: true, width: "default" },
);
</script>

<template>
  <div
    :class="[
      'co-page',
      {
        'co-page--flush': !padded,
        'co-page--full': width === 'full',
        'co-page--static': !scroll,
      },
    ]"
  >
    <div v-if="$slots.header" class="co-page__header">
      <slot name="header" />
    </div>
    <div class="co-page__body">
      <div class="co-page__inner">
        <slot />
      </div>
    </div>
  </div>
</template>

<style scoped>
.co-page {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  min-width: 0;
}

/* 页头常驻，不参与滚动。 */
.co-page__header {
  flex-shrink: 0;
  padding: var(--copper-page-pad-y) var(--copper-page-pad-x) 0;
}

/* 滚动容器：只有这里允许滚动，页面内的 sticky 以它为参照。 */
.co-page__body {
  flex: 1;
  min-height: 0;
  min-width: 0;
  overflow-x: hidden;
  overflow-y: auto;
  padding: var(--copper-page-pad-y) var(--copper-page-pad-x);
  overscroll-behavior: contain;
}

.co-page__inner {
  width: 100%;
  max-width: var(--copper-page-max);
  margin-inline: auto;
}

.co-page--full .co-page__inner {
  max-width: none;
}

/* 页头存在时，正文顶部只留一级间距，避免与页头留白叠成双倍。 */
.co-page__header + .co-page__body {
  padding-top: var(--copper-space-4);
}

.co-page--flush .co-page__header,
.co-page--flush .co-page__body {
  padding: 0;
}

/* 自带滚动容器的页面：正文高度受控，滚动交给内部。 */
.co-page--static .co-page__body {
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.co-page--static .co-page__inner {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

/* 窄窗口收一档留白；移动端再收一档（横屏手机的有效宽度接近桌面窄窗口，
 * 但触控目标更大，留白过小会让边缘点击区贴到屏幕圆角）。 */
@media (max-width: 900px) {
  .co-page__header {
    padding: var(--copper-space-5) var(--copper-space-5) 0;
  }

  .co-page__body {
    padding: var(--copper-space-5) var(--copper-space-5);
  }
}

@media (max-width: 620px) {
  .co-page__header {
    padding: var(--copper-space-4) var(--copper-space-4) 0;
  }

  .co-page__body {
    padding: var(--copper-space-4) var(--copper-space-4);
  }
}

html[data-form-factor="mobile"] .co-page__header {
  padding: var(--copper-space-4) var(--copper-space-4) 0;
}

html[data-form-factor="mobile"] .co-page__body {
  padding: var(--copper-space-4) var(--copper-space-4);
}
</style>
