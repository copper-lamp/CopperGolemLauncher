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
// 颜色用 MC 原版调色板而不是主题色：这些颜色是内容作者写进名字里的信息，
// 跟着主题变会让"红名 = 敌对"之类的约定失效。

import { computed } from "vue";

/** MC 原版 16 色。 */
const COLORS: Record<string, string> = {
  "0": "#000000",
  "1": "#0000AA",
  "2": "#00AA00",
  "3": "#00AAAA",
  "4": "#AA0000",
  "5": "#AA00AA",
  "6": "#FFAA00",
  "7": "#AAAAAA",
  "8": "#555555",
  "9": "#5555FF",
  a: "#55FF55",
  b: "#55FFFF",
  c: "#FF5555",
  d: "#FF55FF",
  e: "#FFFF55",
  f: "#FFFFFF",
};

interface Run {
  text: string;
  color: string | null;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  strike: boolean;
}

function emptyStyle(): Omit<Run, "text"> {
  return { color: null, bold: false, italic: false, underline: false, strike: false };
}

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
      if (COLORS[code]) style.color = COLORS[code];
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

const props = defineProps<{ text: string }>();

const runs = computed(() => parse(props.text));

/** 没有任何样式时走纯文本路径，少一层 span 包裹（列表里可能几百个节点）。 */
const plain = computed(() => {
  if (runs.value.length === 0) return true;
  if (runs.value.length > 1) return false;
  const only = runs.value[0];
  return !only.color && !only.bold && !only.italic && !only.underline && !only.strike;
});

function styleOf(run: Run): Record<string, string> {
  const style: Record<string, string> = {};
  if (run.color) style.color = run.color;
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
  <span v-else>
    <span v-for="(run, index) in runs" :key="index" :style="styleOf(run)">{{ run.text }}</span>
  </span>
</template>
