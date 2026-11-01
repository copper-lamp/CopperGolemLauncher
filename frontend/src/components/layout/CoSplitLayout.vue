<script setup lang="ts">
// 左右分栏骨架（V3）：主栏 + 副栏，副栏默认吸顶，窄容器自动落为上下堆叠。
//
// 为什么用容器查询而不是窗口媒体查询：
// 副栏宽度是「内容区宽度」的函数，不是「窗口宽度」的函数。窗口宽度里还夹着左导航
// （64px）与页面留白，用窗口断点会在不同外壳形态（桌面 / 移动横屏）下错位。
//
// 为什么容器查询要再套一层：CSS 容器不能查询自身。`.co-split` 只负责建立容器，
// 网格与断点都落在 `.co-split__inner` 上。
//
// 断点为版面常量，刻意不放进主题令牌：它不随主题 / 用户设置变化，放进令牌会被误当成
// 可调参数。窗口最小 960px 时内容区可用宽度约 840px，桌面端不会触发折叠。
//
// 调用方契约：
// 1. 副栏内容自己负责底色与内边距，本组件不画卡片、不加分隔线；
// 2. 主栏默认不滚动（滚动由外层 CoPage 承担）；若主栏要独立滚动，自行套滚动容器；
// 3. 副栏吸顶的参照系是外层滚动容器，因此本组件必须放在 CoPage 的滚动区内。

import { computed } from "vue";

const props = withDefaults(
  defineProps<{
    /** 副栏位置：start = 左侧（设置分类导航），end = 右侧（动作 / 筛选栏）。 */
    asidePosition?: "start" | "end";
    /** 副栏宽度（CSS 长度）；为空时用 `--copper-aside-w`，窄容器自动降一档。 */
    asideWidth?: string;
    /** 主副栏间距；为空时用 `--copper-split-gap`。 */
    gap?: string;
    /** 副栏是否吸顶（随页面滚动固定在视口内）。 */
    sticky?: boolean;
    /** 副栏是否内部滚动；开启后副栏高度受滚动区限制。 */
    asideScroll?: boolean;
    /** 折叠成单列时副栏是否排到主栏之前（筛选栏这类要先于结果出现的控件）。 */
    stackAsideFirst?: boolean;
  }>(),
  {
    asidePosition: "end",
    asideWidth: "",
    gap: "",
    sticky: true,
    asideScroll: false,
    stackAsideFirst: false,
  },
);

const style = computed<Record<string, string>>(() => {
  const vars: Record<string, string> = {};
  if (props.asideWidth) vars["--co-split-aside-w"] = props.asideWidth;
  if (props.gap) vars["--co-split-gap"] = props.gap;
  return vars;
});
</script>

<template>
  <div class="co-split" :style="style">
    <div
      :class="[
        'co-split__inner',
        `co-split__inner--${asidePosition}`,
        { 'co-split__inner--stack-aside-first': stackAsideFirst },
      ]"
    >
      <div class="co-split__main">
        <slot />
      </div>
      <aside
        :class="[
          'co-split__aside',
          {
            'co-split__aside--sticky': sticky,
            'co-split__aside--scroll': asideScroll,
          },
        ]"
      >
        <slot name="aside" />
      </aside>
    </div>
  </div>
</template>

<style scoped>
/* 只建立尺寸容器，不参与网格；网格与断点在 .co-split__inner 上。 */
.co-split {
  container-type: inline-size;
  width: 100%;
  min-width: 0;
}

.co-split__inner {
  display: grid;
  grid-template-columns: minmax(0, 1fr) var(--co-split-aside-w, var(--copper-aside-w));
  gap: var(--co-split-gap, var(--copper-split-gap));
  align-items: start;
}

/* 左副栏：DOM 顺序仍是主栏在前，视觉顺序由网格列决定，
 * 这样折叠成单列时主栏天然保持在前，不必重排 DOM。 */
.co-split__inner--start {
  grid-template-columns: var(--co-split-aside-w, var(--copper-aside-w)) minmax(0, 1fr);
}

.co-split__inner--start .co-split__main {
  grid-column: 2;
  grid-row: 1;
}

.co-split__inner--start .co-split__aside {
  grid-column: 1;
  grid-row: 1;
}

.co-split__main {
  min-width: 0;
}

.co-split__aside {
  min-width: 0;
}

/* 吸顶：参照外层滚动容器；align-self 让副栏不被网格行高拉伸，
 * 否则 sticky 会失去可移动空间（元素与行同高，没有「粘住」的余地）。 */
.co-split__aside--sticky {
  position: sticky;
  top: 0;
  align-self: start;
}

/* 副栏内部滚动：高度按窗口减去标题栏与页面上下留白，避免整栏被推到视口外。 */
.co-split__aside--scroll {
  max-height: calc(
    100vh - var(--copper-titlebar-h) - 2 * var(--copper-page-pad-y)
  );
  overflow-y: auto;
  overscroll-behavior: contain;
}

/* 窄一档：副栏先收窄，主栏仍保持可读宽度。 */
@container (max-width: 980px) {
  .co-split__inner {
    grid-template-columns:
      minmax(0, 1fr)
      var(--co-split-aside-w, var(--copper-aside-w-narrow));
  }

  .co-split__inner--start {
    grid-template-columns:
      var(--co-split-aside-w, var(--copper-aside-w-narrow))
      minmax(0, 1fr);
  }
}

/* 折叠：单列堆叠，副栏取消吸顶与内部滚动。
 * 左副栏（如设置分类导航）堆到内容上方，符合「先选分类、再看内容」的阅读顺序；
 * 右副栏（动作 / 筛选）保持在后，不打断主内容。 */
@container (max-width: 760px) {
  .co-split__inner,
  .co-split__inner--start {
    grid-template-columns: minmax(0, 1fr);
  }

  .co-split__main,
  .co-split__inner--start .co-split__main {
    grid-column: 1;
    grid-row: auto;
  }

  .co-split__aside,
  .co-split__inner--start .co-split__aside {
    grid-column: 1;
    grid-row: auto;
  }

  .co-split__inner--start .co-split__aside,
  .co-split__inner--stack-aside-first .co-split__aside {
    order: -1;
  }

  .co-split__aside--sticky {
    position: static;
  }

  .co-split__aside--scroll {
    max-height: none;
    overflow: visible;
  }
}
</style>
