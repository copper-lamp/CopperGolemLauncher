<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { AnimatePresence, motion } from "motion-v";
import { motionEnabled, motionDurations } from "../composables/useVisualPreferences";
import { useSettings } from "../composables/useSettings";

const route = useRoute();
const { loaded: settingsLoaded } = useSettings();
const reduceAnimation = ref(!settingsLoaded.value || !motionEnabled.value);
const routeKey = computed(() => route.name ?? route.matched[route.matched.length - 1]?.path ?? route.path);
watch([motionEnabled, settingsLoaded], ([enabled, loaded]) => {
  reduceAnimation.value = !enabled || !loaded;
}, { immediate: true });

const pageTransition = computed(() => ({
  duration: motionDurations.pageEnter.value / 1000,
  ease: [0.22, 1.28, 0.42, 1] as const,
}));
const pageExitTransition = computed(() => ({
  duration: motionDurations.pageExit.value / 1000,
  ease: "easeIn" as const,
}));
</script>

<template>
  <RouterView v-slot="{ Component }">
    <component :is="Component" v-if="reduceAnimation" :key="routeKey" class="app-route" />
    <AnimatePresence v-else mode="wait" :initial="false">
      <motion.div
        :key="routeKey"
        class="app-route"
        :initial="{ opacity: 0, y: 8, scale: 0.99 }"
        :animate="{ opacity: 1, y: 0, scale: 1, transition: pageTransition }"
        :exit="{ opacity: 0, y: -4, transition: pageExitTransition }"
      >
        <component :is="Component" />
      </motion.div>
    </AnimatePresence>
  </RouterView>
</template>

<style scoped>
.app-route {
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
}
</style>
