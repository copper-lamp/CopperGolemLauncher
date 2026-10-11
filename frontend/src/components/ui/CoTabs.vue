<script setup lang="ts">
// 标签条原语（内核通用组件）：只渲染标签条，不含面板。
//
// 视觉母题取自浏览器标签页：选中项与相邻面板**同色**、无描边、直接连接，接缝处是
// 方角；接缝两端各切出一个**等边直角三角形**，直角顶点就落在接缝顶点上，斜边朝
// 标签条内侧张开 —— 于是选中项沿着面板边缘「外翻」出去。
//
// 为什么不画描边、不加底色差（试过，都不对）：
// - **描边必须去掉**。选中项与面板同色，接缝那一侧若留 1px 线，线上会有两条互相
//   咬合的边，读起来是「贴上去」而不是「长出来」。
// - **底色差是伪解**。让选中项比面板略深一点确实能画出 Chrome 的圆角，但那就变成
//   「标签浮在面板上」的卡片语言了，与本页「三部分是一整块」的版面约束冲突。
//   去掉描边后，白色块在页面底色上的分界本身就是轮廓线，斜边无需再描一次。
//
// 三角形的形状与动画：
// - 每个标签都带两个三角形伪元素（接缝的两个端点各一个），默认 `clip-path` 收拢成
//   端点上的一个点（不可见）；选中项的三角形才展开成直角三角形。
// - `clip-path: polygon()` 的顶点是可插值的，所以切换标签时两个三角形**各自在原地
//   张开 / 收回**，中间态自然衔接 —— 不需要任何位移或位移补间，也就不会出现「白色块
//   飞过去」的廉价感。选中项 `z-index` 抬升，张开的三角形压在相邻标签之上。
// - 填充色也一起过渡（`transparent` → 面板同色），张开过程是「长出来」而不是
//   「闪出来」。
//
// 定位：`absolute` 的包含块是**内边距盒**，四边都受标签自身尺寸影响。
// - 横向（面板在下，咬合下边）：三角形贴在标签左下 / 右下角**外侧**，沿左边（或右边）
//   向左 / 右伸出、沿下边向下伸出。直角顶点在标签角上，斜边连接「标签侧边上距底
//   一个角半径处」与「面板上边框上距该角一个角半径处」，两端分别落在标签边界与
//   面板边界上，接缝因此连续。
// - 纵向（面板在右，咬合右边）：上 / 下端三角形同理，贴在标签右上 / 右下角外侧。
//
// 调用方契约（重要）：
// 1. 面板 `background` 必须是 `var(--copper-surface)`，被咬合的那条边为
//    `1px solid var(--copper-border)`；
// 2. 条带须朝面板方向负偏移 1px，与被咬合的面板之间不能有其它间隙；
// 3. 三角形会越出条带边界，而**条带的祖先若带 `overflow`，三角形的斜边就会被裁掉**。
//    纵向条带常见于可滚动的侧栏里，此时必须给滚动容器在**所有会外伸的方向**上
//    补足一个 `--copper-radius-md` 的内边距。
//
// 选中态只靠「同色块 + 斜切三角」表达，不另加指示条；
// 复杂标签内容（图片、多行）用 `#item` 插槽替换默认的 图标 + 文本。

import type { Component } from "vue";

const props = withDefaults(
  defineProps<{
    modelValue: string;
    /** 标签项；`icon` 为 @lucide/vue 图标组件。 */
    items: Array<{ value: string; label: string; icon?: Component }>;
    direction?: "vertical" | "horizontal";
  }>(),
  { direction: "horizontal" },
);

const emit = defineEmits<{ "update:modelValue": [value: string] }>();

function select(value: string) {
  if (value === props.modelValue) return;
  emit("update:modelValue", value);
}
</script>

<template>
  <div :class="['co-tabs', `co-tabs--${direction}`]" role="tablist">
    <button
      v-for="item in items"
      :key="item.value"
      data-co-tab
      type="button"
      role="tab"
      :aria-selected="item.value === modelValue"
      :class="['co-tabs__tab', { 'co-tabs__tab--active': item.value === modelValue }]"
      @click="select(item.value)"
    >
      <slot name="item" :item="item" :active="item.value === modelValue">
        <component :is="item.icon" v-if="item.icon" :size="15" />
        <span class="co-tabs__label">{{ item.label }}</span>
      </slot>
    </button>
  </div>
</template>

<style scoped>
.co-tabs {
  /* 外扩量 = 斜边在标签边 / 面板边各让出的一条直角边长度，
     也决定三角形补块见方。 */
  --co-tabs-corner: var(--copper-radius-md);
  position: relative;
  z-index: 1; /* 抬升条带，使选中项能压住相邻面板的边框 */
}

.co-tabs--vertical {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.co-tabs--horizontal {
  display: flex;
  flex-direction: row;
  gap: 2px;
}

.co-tabs__tab {
  position: relative;
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  /* 无描边：轮廓线由「白色块 vs 页面底色」的分界自然形成，
     再画一圈线只会让选中项看起来是个可以点的按钮。 */
  border: none;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.co-tabs--vertical .co-tabs__tab {
  width: 100%;
  padding: var(--copper-space-2) var(--copper-space-3);
}

.co-tabs--horizontal .co-tabs__tab {
  padding: var(--copper-space-2) var(--copper-space-4);
}

.co-tabs__tab:hover:not(.co-tabs__tab--active) {
  background: var(--copper-hover);
  color: var(--copper-text);
}

/*
 * 选中项：与面板同色、无描边、咬合侧取直角 —— 接缝就是一个方角。
 * z-index 抬升，让下面两个张开的三角形压在相邻标签之上（浏览器标签页同款：
 * 活动标签会「咬」掉相邻标签的角）。
 */
.co-tabs__tab--active {
  z-index: 1;
  background: var(--copper-surface);
  color: var(--copper-text);
  font-weight: 500;
}

/* 纵向：面板在右，咬合右边 —— 上端直角、左下直；下端直角、右上直。 */
.co-tabs--vertical .co-tabs__tab--active {
  border-radius: var(--co-tabs-corner) 0 0 var(--co-tabs-corner);
}

/* 横向：面板在下，咬合下边 —— 上侧两角圆角，下侧两角取直角。 */
.co-tabs--horizontal .co-tabs__tab--active {
  border-radius: var(--co-tabs-corner) var(--co-tabs-corner) 0 0;
}

/*
 * 外翻三角形：见方补块，直角顶点落在选中项咬合端的角顶点上。
 *
 * `clip-path` 的三个顶点用百分比写，右下角 = 直角顶点，另外两点各在补块的对角，
 * 斜边即连接它们。收拢态把三点全压在直角顶点上 —— 退化成零面积但**仍然是同一个
 * 形状**，所以 clip-path 的顶点插值在两个状态之间是连续的，动画不会出现「三角形
 * 从无到有」的跳变。
 *
 * 展开态的斜边两端分别落在：标签侧边上距该角一个 `--co-tabs-corner` 处，
 * 与面板边界上距该角一个 `--co-tabs-corner` 处。两条边界都是现成的轮廓线，
 * 斜边接上去，接缝因此连续。
 */
.co-tabs__tab::before,
.co-tabs__tab::after {
  content: "";
  position: absolute;
  z-index: -1; /* 贴在标签背景之下，避免压住标签文字 */
  width: var(--co-tabs-corner);
  height: var(--co-tabs-corner);
  background: transparent;
  pointer-events: none;
  /* 收拢态：退化成端点上的一个点 */
  clip-path: polygon(100% 100%, 100% 100%, 100% 100%);
  transition:
    clip-path var(--copper-duration) var(--copper-easing),
    background-color var(--copper-duration) var(--copper-easing);
}

/*
 * 横向：贴在下缘之外 —— 左端补块跨过标签左边与下边，直角顶点 = 标签左下角；
   右端补块跨过标签右边与下边，直角顶点 = 标签右下角。
   元素正好落在「角顶点向左/右一个 r、向上一个 r」的那一格里，斜边即对角线。 */
.co-tabs--horizontal .co-tabs__tab::before {
  left: calc(0px - var(--co-tabs-corner) - 1px);
  bottom: -1px;
}

.co-tabs--horizontal .co-tabs__tab::after {
  right: calc(0px - var(--co-tabs-corner));
  bottom: -1px;
}

.co-tabs--horizontal .co-tabs__tab--active::before {
  /* 直角顶点 = 元素左下（= 标签左下角），斜边从右上连到左上。 */
  clip-path: polygon(0% 100%, 100% 100%, 0% 0%);
  background: var(--copper-surface);
}

.co-tabs--horizontal .co-tabs__tab--active::after {
  /* 直角顶点 = 元素右下（= 标签右下角），斜边从左下连到右上。 */
  clip-path: polygon(100% 100%, 0% 100%, 100% 0%);
  background: var(--copper-surface);
}

/* 纵向：贴在右缘之外 —— 上端补块跨过标签上边与右边，直角顶点 = 标签右上角；
   下端补块跨过标签下边与右边，直角顶点 = 标签右下角。 */
.co-tabs--vertical .co-tabs__tab::before {
  right: calc(0px - var(--co-tabs-corner));
  top: calc(0px - var(--co-tabs-corner));
}

.co-tabs--vertical .co-tabs__tab::after {
  right: calc(0px - var(--co-tabs-corner));
  bottom: calc(0px - var(--co-tabs-corner));
}

.co-tabs--vertical .co-tabs__tab--active::before {
  /* 直角顶点 = 元素左下（= 标签右上角），斜边从左上连到右下。 */
  clip-path: polygon(0% 100%, 0% 0%, 100% 100%);
  background: var(--copper-surface);
}

.co-tabs--vertical .co-tabs__tab--active::after {
  /* 直角顶点 = 元素右下（= 标签右下角），斜边从右上连到左下。 */
  clip-path: polygon(100% 100%, 100% 0%, 0% 100%);
  background: var(--copper-surface);
}

.co-tabs__label {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

/* 窄屏标签页更矮，三角形按比例收一档，否则斜边显得笨重。 */
@media (max-width: 720px) {
  .co-tabs {
    --co-tabs-corner: 7px;
  }
}
</style>
