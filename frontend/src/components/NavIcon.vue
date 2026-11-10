<script setup lang="ts">
import type { Component } from "vue";
import { PhHouse, PhPackage, PhDownloadSimple, PhGear, PhGameController } from "@phosphor-icons/vue";
const props = defineProps<{ id: string; active: boolean; fallback?: Component; size?: number }>();
const icons: Record<string, Component> = { home: PhHouse, "content-download": PhPackage, "game-download": PhGameController, downloads: PhDownloadSimple, settings: PhGear };
</script>
<template>
  <component :is="icons[props.id] ?? fallback" :size="size ?? 20" :weight="active ? 'fill' : 'regular'" :class="{ 'nav-icon--active-fallback': active && !icons[id] }" aria-hidden="true" />
</template>
<style scoped>
/* Third-party icon components stay compatible without imposing invalid SVG fills. */
.nav-icon--active-fallback { filter: drop-shadow(0 0 1px currentColor); stroke-width: 3; }
</style>
