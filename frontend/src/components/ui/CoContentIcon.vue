<script setup lang="ts">
import { computed, ref, watch } from "vue";

const props = withDefaults(defineProps<{
  name: string;
  src?: string | null;
  /** Only provide an avatar when the source identifies a real author account. */
  avatar?: string | null;
}>(), { src: null, avatar: null });
const failed = ref<string[]>([]);
watch(() => [props.src, props.avatar], () => { failed.value = []; });
const image = computed(() => [props.src, props.avatar].find((url) => url && !failed.value.includes(url)) || null);
const initial = computed(() => Array.from(props.name.trim())[0]?.toLocaleUpperCase() || "?");
const tone = computed(() => {
  let hash = 0;
  for (const char of props.name) hash = (Math.imul(hash, 31) + char.codePointAt(0)!) | 0;
  return ["source-lip", "texture-pack", "behavior-pack", "shader", "release", "beta"][Math.abs(hash) % 6];
});
const colors = computed(() => ({
  background: `var(--copper-badge-${tone.value})`,
  color: `var(--copper-badge-${tone.value}-fg)`,
}));
function onError() { if (image.value) failed.value.push(image.value); }
</script>

<template>
  <span class="co-content-icon" :style="colors" aria-hidden="true">
    <img v-if="image" :key="image" :src="image" alt="" loading="lazy" draggable="false" referrerpolicy="no-referrer" @error="onError" />
    <span v-else>{{ initial }}</span>
  </span>
</template>

<style scoped>
.co-content-icon { display:flex;align-items:center;justify-content:center;width:100%;height:100%;overflow:hidden;border-radius:inherit;font-size:1.25em;font-weight:700;line-height:1; }
.co-content-icon img { width:100%;height:100%;object-fit:cover; }
</style>
