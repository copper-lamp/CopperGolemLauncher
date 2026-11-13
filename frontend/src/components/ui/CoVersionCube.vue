<script setup lang="ts">
defineProps<{ channel: string; label: string }>();
</script>

<template>
  <svg class="co-version-cube" :class="channel === 'release' ? 'is-release' : 'is-preview'" viewBox="0 0 64 68" role="img" :aria-label="label">
    <title>{{ label }}</title>
    <path class="cube-top" d="M32 4 58 18 32 32 6 18Z" />
    <path class="cube-left" d="M6 18 32 32 32 60 6 46Z" />
    <path class="cube-right" d="M32 32 58 18 58 46 32 60Z" />
    <text x="32" y="32" text-anchor="middle" dominant-baseline="central">{{ channel === 'release' ? 'R' : 'P' }}</text>
  </svg>
</template>

<style scoped>
.co-version-cube { flex-shrink:0;width:38px;height:40px;overflow:visible;color:#24784b;filter:drop-shadow(0 3px 4px rgb(24 35 48 / .12)); }
.cube-top { fill:#55cf8b; }.cube-left { fill:#22995b; }.cube-right { fill:#14713e; }
.co-version-cube text { fill:#fff;font:800 34px system-ui,sans-serif;paint-order:stroke;stroke:rgb(0 0 0 / .12);stroke-width:.7px; }
.is-preview .cube-top { fill:#64adfa; }.is-preview .cube-left { fill:#2e7dd5; }.is-preview .cube-right { fill:#1d55a1; }
:global(html[data-theme="dark"]) .co-version-cube { filter:drop-shadow(0 3px 5px rgb(0 0 0 / .24)); }
</style>
