<script setup lang="ts">
import { Languages, LoaderCircle, Undo2 } from "@lucide/vue";
import CoButton from "./ui/CoButton.vue";
import { useTranslationMessages } from "../services/translation-messages";
defineProps<{ busy: boolean; translated: boolean; disabled?: boolean; error?: string }>();
const emit = defineEmits<{ toggle: [] }>();
const m = useTranslationMessages();
</script>
<template>
  <div class="translation-action">
    <CoButton variant="secondary" :disabled="disabled && !busy" :title="busy ? m.translating : translated ? m.original : m.translate" :aria-label="busy ? m.translating : translated ? m.original : m.translate" :aria-pressed="translated" @click="emit('toggle')">
      <LoaderCircle v-if="busy" :size="15" class="translation-spin" /><Undo2 v-else-if="translated" :size="15" /><Languages v-else :size="15" /><span>{{ busy ? m.translating : translated ? m.original : '英/中' }}</span>
    </CoButton>
    <span v-if="error" class="translation-error" role="alert" :title="error">{{ m.failed }} · {{ error }}</span>
  </div>
</template>
<style scoped>
.translation-action { display: flex; align-items: center; flex-wrap: wrap; gap: 10px; }
.translation-error { max-width: 420px; color: var(--copper-danger); font-size: var(--copper-font-size-xs); overflow-wrap: anywhere; }
.translation-spin { animation: translation-spin .9s linear infinite; }
@keyframes translation-spin { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .translation-spin { animation: none; } }
</style>
