<script setup lang="ts">
// Minecraft 文本格式码（§）渲染。
//
// 存档名、服务器名、模组名都可能带 `§` 样式码（`§a` `§l` 等）。原样输出会把
// "§a" 这类控制序列直接摆在用户面前，所以这里解析成带样式的片段：
// - `§0`–`§f`：颜色；
// - `§l` 粗体 / `§o` 斜体 / `§n` 下划线 / `§m` 删除线；
// - `§r` 重置；
// - `§k`（乱码）与未知码：丢弃样式码本身，不保留控制字符。
//
// **颜色不做逐主题直出**：MC 的 16 色是按"游戏内深色背景"设计的，原样搬到白底上
// 会直接失效（`§f` 白字、`§e` 亮黄在白底上几乎看不见）。因此颜色落在 `mc--<code>`
// 类上，由下面的样式按主题给出一套可读的等价色：深色主题把暗色提亮，浅色主题把
// 亮色压深，色相保持不变（"红名 = 敌对"这类约定不被破坏）。

import { computed } from "vue";

interface Run {
  text: string;
  /** MC 颜色码（0-f）；为空表示沿用父级颜色。 */
  code: string | null;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  strike: boolean;
}

function emptyStyle(): Omit<Run, "text"> {
  return { code: null, bold: false, italic: false, underline: false, strike: false };
}

const COLOR_CODES = "0123456789abcdef";

function parse(input: string): Run[] {
  const runs: Run[] = [];
  let style = emptyStyle();
  let buffer = "";

  const flush = () => {
    if (!buffer) return;
    runs.push({ text: buffer, ...style });
    buffer = "";
  };

  for (let i = 0; i < input.length; i += 1) {
    const ch = input[i];
    if (ch === "§" && i + 1 < input.length) {
      const code = input[i + 1].toLowerCase();
      i += 1;
      flush();
      if (COLOR_CODES.includes(code)) style.code = code;
      else if (code === "l") style.bold = true;
      else if (code === "o") style.italic = true;
      else if (code === "n") style.underline = true;
      else if (code === "m") style.strike = true;
      else if (code === "r") style = emptyStyle();
      // §k 与未知码：只吞掉控制字符。
      continue;
    }
    buffer += ch;
  }
  flush();
  return runs;
}

const props = withDefaults(
  defineProps<{
    text: string;
    /** 文本叠在深色底上（封面大卡）：强制用深色主题那套色，浅色主题下也保持可读。 */
    onDark?: boolean;
  }>(),
  { onDark: false },
);

const runs = computed(() => parse(props.text));

/** 没有任何样式时走纯文本路径，少一层 span 包裹（列表里可能几百个节点）。 */
const plain = computed(() => {
  if (runs.value.length === 0) return true;
  if (runs.value.length > 1) return false;
  const only = runs.value[0];
  return !only.code && !only.bold && !only.italic && !only.underline && !only.strike;
});

function styleOf(run: Run): Record<string, string> {
  const style: Record<string, string> = {};
  if (run.bold) style.fontWeight = "700";
  if (run.italic) style.fontStyle = "italic";
  const deco: string[] = [];
  if (run.underline) deco.push("underline");
  if (run.strike) deco.push("line-through");
  if (deco.length) style.textDecoration = deco.join(" ");
  return style;
}
</script>

<template>
  <span v-if="plain">{{ text.replace(/§./g, "") }}</span>
  <span v-else :class="['mc', { 'mc-on-dark': onDark }]">
    <span
      v-for="(run, index) in runs"
      :key="index"
      :class="run.code ? `mc--${run.code}` : ''"
      :style="styleOf(run)"
    >{{ run.text }}</span>
  </span>
</template>

<style scoped>
/* 两套调色板都只改亮度、不改色相 —— 名字里的"颜色语义"必须保留。 */
.mc {
  --mc-0: #8b949e;
  --mc-1: #6e8bff;
  --mc-2: #3fb950;
  --mc-3: #39c5cf;
  --mc-4: #f85149;
  --mc-5: #d2a8ff;
  --mc-6: #e3b341;
  --mc-7: #9aa4b2;
  --mc-8: #7d8590;
  --mc-9: #79c0ff;
  --mc-a: #7ee787;
  --mc-b: #56d4dd;
  --mc-c: #ff7b72;
  --mc-d: #ff9bce;
  --mc-e: #e3b341;
  --mc-f: #f0f6fc;
}

/* 浅色主题：保持色相，把过亮的 a-f 与白字压深到白底上可读。
 * `.mc-on-dark`（叠在封面图上）不吃这套 —— 那种底本来就是深的。 */
html[data-theme="light"] .mc:not(.mc-on-dark) {
  --mc-0: #1f2328;
  --mc-1: #2020b0;
  --mc-2: #0f7b0f;
  --mc-3: #007a7a;
  --mc-4: #b42318;
  --mc-5: #8b1a8b;
  --mc-6: #8a5a00;
  --mc-7: #57606a;
  --mc-8: #424a53;
  --mc-9: #2f4fcf;
  --mc-a: #1a7f37;
  --mc-b: #0b6f7a;
  --mc-c: #cf222e;
  --mc-d: #a21f8f;
  --mc-e: #7d5a00;
  --mc-f: #1f2328;
}

.mc--0 { color: var(--mc-0); }
.mc--1 { color: var(--mc-1); }
.mc--2 { color: var(--mc-2); }
.mc--3 { color: var(--mc-3); }
.mc--4 { color: var(--mc-4); }
.mc--5 { color: var(--mc-5); }
.mc--6 { color: var(--mc-6); }
.mc--7 { color: var(--mc-7); }
.mc--8 { color: var(--mc-8); }
.mc--9 { color: var(--mc-9); }
.mc--a { color: var(--mc-a); }
.mc--b { color: var(--mc-b); }
.mc--c { color: var(--mc-c); }
.mc--d { color: var(--mc-d); }
.mc--e { color: var(--mc-e); }
.mc--f { color: var(--mc-f); }
</style>
