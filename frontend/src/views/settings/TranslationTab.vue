<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoSkeleton from "../../components/ui/CoSkeleton.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import { useTranslationMessages, useTranslationError } from "../../services/translation-messages";
import { translationConfig, translationSaveConfig, translationTest, translationClearCache, type TranslationConfig } from "../../services/translation";
const m = useTranslationMessages();
const errorMessage = useTranslationError();
const config = ref<TranslationConfig | null>(null);
const busy = ref(false);
const loading = ref(true);
const error = ref("");
const status = ref("");
const languageOptions = computed(() => [{ value: "auto", label: m.value.follow }, { value: "zh-CN", label: "简体中文" }, { value: "zh-TW", label: "繁體中文" }, { value: "en", label: "English" }, { value: "ja", label: "日本語" }, { value: "ko", label: "한국어" }, { value: "de", label: "Deutsch" }, { value: "fr", label: "Français" }, { value: "es", label: "Español" }, { value: "ru", label: "Русский" }]);
async function load() {
  loading.value = true; error.value = "";
  try { config.value = await translationConfig(); }
  catch (e) { error.value = errorMessage(e); }
  finally { loading.value = false; }
}
async function action(kind: "save" | "test" | "clear") {
  if (!config.value || busy.value) return;
  busy.value = true; error.value = ""; status.value = "";
  try {
    if (kind === "save") {
      // Preserve normalized legacy provider settings; this form edits only language/cache.
      await translationSaveConfig(config.value);
      config.value = await translationConfig();
      status.value = m.value.saved;
    } else if (kind === "test") {
      const result = await translationTest(config.value.provider);
      status.value = `${m.value.testOk} · ${result.elapsed_ms} ms · ${result.text}`;
    } else { await translationClearCache(); status.value = m.value.cleared; }
  } catch (e) { error.value = errorMessage(e); }
  finally { busy.value = false; }
}
onMounted(load);
</script>

<template>
  <section class="translation-settings" :aria-busy="busy || loading">
    <h2>{{ m.title }}</h2><p class="hint">{{ m.description }}</p>
    <CoSkeleton v-if="loading" variant="settings" :rows="3" :label="m.loading" />
    <form v-else-if="config" @submit.prevent="action('save')">
      <label>{{ m.target }}<CoSelect v-model="config.target_language" :options="languageOptions" :disabled="busy" /></label>
      <label>{{ m.cache }}<input v-model.number="config.cache_days" class="number" type="number" min="0" max="365" :disabled="busy" /><small>{{ m.cacheHint }}</small></label>
      <p class="hint">{{ m.defaultHint }}</p>
      <p class="hint">{{ m.notice }}</p>
      <div class="actions"><CoButton type="submit" :disabled="busy">{{ m.save }}</CoButton><CoButton variant="secondary" :disabled="busy" @click="action('test')">{{ m.test }}</CoButton><CoButton variant="secondary" :disabled="busy" @click="action('clear')">{{ m.clear }}</CoButton></div>
    </form>
    <p v-if="error" role="alert" class="error">{{ error }}</p><p v-if="status" role="status" class="hint">{{ status }}</p>
    <CoButton v-if="!config && !loading" variant="secondary" @click="load">{{ m.retry }}</CoButton>
  </section>
</template>
<style scoped>
.translation-settings { padding: var(--copper-space-5); background: var(--copper-surface); border-radius: var(--copper-radius-card); box-shadow: var(--copper-shadow-card); }
h2 { margin: 0 0 12px; font-size: var(--copper-font-size-lg); }
form { display: grid; gap: 16px; }
label { display: grid; gap: 8px; color: var(--copper-text); }
label > :deep(.co-select) { max-width: 560px; }
.hint, small { color: var(--copper-text-secondary); font-size: var(--copper-font-size-sm); overflow-wrap: anywhere; }
.number { padding: 10px 12px; color: var(--copper-text); background: var(--copper-bg); border: 0; border-radius: var(--copper-radius-control); font: inherit; max-width: 180px; }
.actions { display: flex; gap: 10px; flex-wrap: wrap; }
.error { color: var(--copper-danger); overflow-wrap: anywhere; }
</style>
