<script setup lang="ts">
// 标签条原语（内核通用组件）：只渲染标签条，不含面板。
//
// 视觉母题取自浏览器标签页 —— 选中项与相邻面板同色、被咬合那一侧的边框断开，
// 形成一条无缝接缝；接缝两端是**向外张开的倒角**（外弧，即"反向圆角"），
// 让选中项沿着面板边缘平滑地铺开。
//
// 实现方式：
// 1. 调用方把条带朝面板方向负偏移 1px（纵向 `margin-right`、横向 `margin-bottom`），
//    选中项自身那条边框（取面板同色）正好压住面板的 1px 边框；
// 2. 选中项被咬合那一侧的两个端角取直角，并在其**外侧**各贴一个同色方块，
//    方块用径向遮罩挖出四分之一圆，露出缺口 —— 即外弧倒角（`--co-tabs-corner`）。
//    遮罩而非渐变色：渐变从透明过渡到实色会经过半透明灰，遮罩只取 alpha，无灰边。
// 3. 倒角方块要压在相邻标签之上，故选中项自身抬升一层。
//
// 方向：
// - `vertical`：纵向条带，咬合右边（面板在右侧），倒角落在右上 / 右下；
// - `horizontal`：横向条带，咬合下边（面板在下方），倒角落在左下 / 右下。
//
// 调用方契约（重要）：
// 1. 面板 `background` 必须是 `var(--copper-surface)`，被咬合的那条边为
//    `1px solid var(--copper-border)`；
// 2. 条带须朝面板方向负偏移 1px，且与被咬合的面板之间不能有其它间隙；
// 3. 条带自身不要设置滚动（需要滚动请在条带外层套 overflow 容器）。
//
// 选中态只靠「同色 + 边框断开」表达，不另加指示条；
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
  --co-tabs-corner: var(--copper-radius-md);
  position: relative;
  z-index: 1; /* 抬升条带，使选中项能压住相邻面板的边框 */
  border: none;
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
  border: 1px solid transparent;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing),
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

.co-tabs__tab--active {
  z-index: 1; /* 倒角方块需压住左右相邻标签 */
  background: var(--copper-surface);
  color: var(--copper-text);
  font-weight: 500;
}

/* 纵向：咬合右边 —— 条带 `margin-right: -1px` 后，选中项那 1px 右边框与面板的
   1px 左边框同处一列，取面板同色即可抹掉接缝；左侧两角仍圆角，右侧两角取直角
   以便外接倒角方块。 */
.co-tabs--vertical .co-tabs__tab--active {
  border-color: var(--copper-border);
  border-right-color: var(--copper-surface);
  border-radius: var(--co-tabs-corner) 0 0 var(--co-tabs-corner);
}

/* 横向：咬合下边 —— 同理，条带 `margin-bottom: -1px` 后选中项的底边框压住
   面板的上边框，取面板同色即抹掉接缝；上侧两角圆角，下侧两角取直角。 */
.co-tabs--horizontal .co-tabs__tab--active {
  border-color: var(--copper-border);
  border-bottom-color: var(--copper-surface);
  border-radius: var(--co-tabs-corner) var(--co-tabs-corner) 0 0;
}

/* 外弧倒角：同色方块压在标签外侧，用径向遮罩挖掉靠近标签的那个四分之一圆，
   留下的部分即从标签边缘向外张开的凹弧。 */
.co-tabs__tab--active::before,
.co-tabs__tab--active::after {
  content: "";
  position: absolute;
  width: var(--co-tabs-corner);
  height: var(--co-tabs-corner);
  background: var(--copper-surface);
  pointer-events: none;
}

/* 纵向：倒角贴在右边框之外（`-1px` 让它与 1px 边框对齐），上下各一。
   上端圆心取方块左下角、下端取左上角，弧线均朝面板方向张开。 */
.co-tabs--vertical .co-tabs__tab--active::before,
.co-tabs--vertical .co-tabs__tab--active::after {
  left: calc(100% - 1px);
}

.co-tabs--vertical .co-tabs__tab--active::before {
  bottom: 100%;
  -webkit-mask: radial-gradient(circle at 0 100%, #0000 calc(var(--co-tabs-corner) - 1px), #000 var(--co-tabs-corner));
  mask: radial-gradient(circle at 0 100%, #0000 calc(var(--co-tabs-corner) - 1px), #000 var(--co-tabs-corner));
}

.co-tabs--vertical .co-tabs__tab--active::after {
  top: 100%;
  -webkit-mask: radial-gradient(circle at 0 0, #0000 calc(var(--co-tabs-corner) - 1px), #000 var(--co-tabs-corner));
  mask: radial-gradient(circle at 0 0, #0000 calc(var(--co-tabs-corner) - 1px), #000 var(--co-tabs-corner));
}

/* 横向：倒角贴在底边之外，左右各一。左端圆心取方块左上角、右端取右上角。 */
.co-tabs--horizontal .co-tabs__tab--active::before,
.co-tabs--horizontal .co-tabs__tab--active::after {
  bottom: 0;
}

.co-tabs--horizontal .co-tabs__tab--active::before {
  right: 100%;
  -webkit-mask: radial-gradient(circle at 0 0, #0000 calc(var(--co-tabs-corner) - 1px), #000 var(--co-tabs-corner));
  mask: radial-gradient(circle at 0 0, #0000 calc(var(--co-tabs-corner) - 1px), #000 var(--co-tabs-corner));
}

.co-tabs--horizontal .co-tabs__tab--active::after {
  left: 100%;
  -webkit-mask: radial-gradient(circle at 100% 0, #0000 calc(var(--co-tabs-corner) - 1px), #000 var(--co-tabs-corner));
  mask: radial-gradient(circle at 100% 0, #0000 calc(var(--co-tabs-corner) - 1px), #000 var(--co-tabs-corner));
}

.co-tabs__label {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
</style>
