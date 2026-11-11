<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from "vue";
const visible = ref(false);
let timer: ReturnType<typeof setTimeout> | undefined;
onMounted(() => { timer = setTimeout(() => { visible.value = true; }, 150); });
onBeforeUnmount(() => { clearTimeout(timer); });
withDefaults(defineProps<{ variant?: "list" | "detail" | "settings" | "home"; rows?: number; label: string }>(), { variant: "list", rows: 3 });
</script>
<template>
  <Transition name="co-loading"><div v-if="visible" class="co-skeleton" :class="`co-skeleton--${variant}`" role="status" :aria-label="label" aria-busy="true">
    <span class="co-skeleton__sr">{{ label }}</span>
    <div v-if="variant === 'detail' || variant === 'home'" class="co-skeleton__hero" aria-hidden="true"><i /><i /><i /></div>
    <div class="co-skeleton__body" aria-hidden="true"><div v-if="variant === 'settings' || variant === 'detail'" class="co-skeleton__aside"><i v-for="n in 4" :key="n" /></div><div class="co-skeleton__rows"><div v-for="n in rows" :key="n" class="co-skeleton__row"><i class="co-skeleton__icon" /><div><i /><i /></div><i class="co-skeleton__action" /></div></div></div>
  </div></Transition>
</template>
<style scoped>
.co-skeleton { width:100%;display:flex;flex-direction:column;gap:20px; }
.co-skeleton__sr { position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%); }
.co-skeleton i { display:block;background:var(--copper-surface-3);border-radius:var(--copper-radius-sm);animation:co-skeleton-pulse 1.5s ease-in-out infinite; }
.co-skeleton__hero { min-height:180px;padding:28px;border-radius:var(--copper-radius-card);background:var(--copper-surface);box-shadow:var(--copper-shadow-card);display:flex;flex-direction:column;justify-content:center;gap:16px; }
.co-skeleton__hero i { height:16px;width:40%; }.co-skeleton__hero i:first-child { height:28px;width:55%; }
.co-skeleton__body { display:flex;gap:20px; }.co-skeleton__rows { flex:1;min-width:0;display:flex;flex-direction:column;gap:12px; }
.co-skeleton__row { display:flex;align-items:center;gap:16px;padding:20px;border-radius:var(--copper-radius-card);background:var(--copper-surface);box-shadow:var(--copper-shadow-card); }
.co-skeleton__row>div { flex:1;display:flex;flex-direction:column;gap:10px; }.co-skeleton__row>div i { height:12px;width:60%; }.co-skeleton__row>div i:first-child { height:16px;width:80%; }
.co-skeleton .co-skeleton__icon { width:44px;height:44px;flex-shrink:0; }.co-skeleton .co-skeleton__action { width:72px;height:30px;border-radius:var(--copper-radius-control); }
.co-skeleton__aside { width:190px;display:flex;flex-direction:column;gap:16px;padding:20px;border-radius:var(--copper-radius-card);background:var(--copper-surface); }.co-skeleton__aside i { height:22px; }
@keyframes co-skeleton-pulse { 0%,100% { opacity:.45; } 50% { opacity:.8; } }
@media(max-width:640px) { .co-skeleton__aside { display:none; }.co-skeleton__row { padding:16px; }.co-skeleton .co-skeleton__action { width:44px; } }
</style>
