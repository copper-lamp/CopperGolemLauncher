<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import NavIcon from "./NavIcon.vue";
import { useDownloadFeedback } from "../composables/useDownloadFeedback";
import { useSettings } from "../composables/useSettings";
defineProps<{ active: boolean; size?: number }>();
const target = ref<HTMLElement | null>(null);
const { unreadCompletedCount, completionPulseSeq, activeCount, registerDownloadTarget } = useDownloadFeedback();
const { get } = useSettings();
const count = computed(() => get<boolean>("download.completion_badge_enabled", true) ? unreadCompletedCount.value : 0);
let animation: Animation | undefined;
watch(completionPulseSeq, () => {
  if (!target.value || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  animation?.cancel();
  animation = target.value.animate([{ transform: "scale(1)" }, { transform: "scale(1.22)" }, { transform: "scale(1)" }], { duration: 460, easing: "ease-out" });
});
onMounted(() => registerDownloadTarget(target.value));
onBeforeUnmount(() => { animation?.cancel(); registerDownloadTarget(null); });
</script>
<template>
  <span ref="target" class="download-nav-icon" :class="{ 'download-nav-icon--working': activeCount > 0 }">
    <NavIcon id="downloads" :active="active" :size="size" />
    <span v-if="count > 0" class="download-nav-icon__badge">{{ count > 99 ? '99+' : count }}</span>
  </span>
</template>
<style scoped>
.download-nav-icon { display: inline-flex; position: relative; }
.download-nav-icon--working::after { content: ''; width: 4px; height: 4px; background: var(--copper-accent); border-radius: 50%; position: absolute; bottom: -5px; left: calc(50% - 2px); }
.download-nav-icon__badge { position: absolute; top: -9px; left: 12px; min-width: 15px; height: 15px; padding: 0 3px; border-radius: 10px; display: flex; align-items: center; justify-content: center; font-size: 10px; line-height: 1; font-weight: 600; color: var(--copper-surface); background: var(--copper-accent); box-shadow: 0 1px 5px color-mix(in srgb, #000 14%, transparent); }
</style>
