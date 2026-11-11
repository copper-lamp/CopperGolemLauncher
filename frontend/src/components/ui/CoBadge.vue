<script setup lang="ts">
// 胶囊浅底彩字；所有色值由深浅主题共享令牌提供。
import { computed } from "vue";
export type BadgeTone = "release" | "beta" | "alpha" | "preview" | "ll-mod" | "downloaded" | "behavior-pack" | "texture-pack" | "shader" | "source-curseforge" | "source-lip" | "source-lla" | "success" | "warning" | "danger" | "accent" | "neutral";
const props = withDefaults(defineProps<{ tone: BadgeTone; size?: "sm" | "md"; label?: string; title?: string }>(), { size: "md", label: undefined, title: undefined });
const style = computed(() => ({ background: `var(--copper-badge-${props.tone})`, color: `var(--copper-badge-${props.tone}-fg)` }));
const visible = computed(() => (props.label ?? "").trim().length > 0);
</script>
<template><span v-if="visible" class="co-badge" :class="`co-badge--${size}`" :style="style" :title="title">{{ label }}</span></template>
<style scoped>
.co-badge { display:inline-flex;align-items:center;flex-shrink:0;padding:2px 9px;border-radius:var(--copper-radius-full);font-size:var(--copper-font-size-xs);font-weight:600;letter-spacing:.3px;line-height:1.5;white-space:nowrap; }
.co-badge--sm { padding:1px 7px;font-size:10px; }
</style>
