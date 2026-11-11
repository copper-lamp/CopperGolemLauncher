<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoSkeleton from "../../components/ui/CoSkeleton.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { useTranslationMessages, useTranslationError } from "../../services/translation-messages";
import { translationProviders, translationConfig, translationSaveConfig, translationTest, translationClearCache, type TranslationConfig, type TranslationProvider } from "../../services/translation";
const m = useTranslationMessages();
const errorMessage = useTranslationError();
const config = ref<TranslationConfig | null>(null);
const providers = ref<TranslationProvider[]>([]);
const key = ref("");
const removeKey = ref(false);
const busy = ref(false);
const loading = ref(true);
const error = ref("");
const status = ref("");
const selected = computed(() => providers.value.find(p => p.id === config.value?.provider));
const options = computed(() => providers.value.map(p => ({ value: p.id, label: `${p.name}${!p.available ? ` · ${m.value.unsupported}` : ""}` })));
const providerConfig = computed(() => {
  if (!config.value) return null;
  return config.value.providers[config.value.provider] ??= { endpoint: selected.value?.endpoint ?? "", model: "", region: "", pro: false };
});
const languageOptions = computed(() => [{ value: "auto", label: m.value.follow }, { value: "zh-CN", label: "简体中文" }, { value: "zh-TW", label: "繁體中文" }, { value: "en", label: "English" }, { value: "ja", label: "日本語" }, { value: "ko", label: "한국어" }, { value: "de", label: "Deutsch" }, { value: "fr", label: "Français" }, { value: "es", label: "Español" }, { value: "ru", label: "Русский" }]);
async function load() {
  loading.value = true;
  try { [providers.value, config.value] = await Promise.all([translationProviders(), translationConfig()]); }
  catch (e) { error.value = errorMessage(e); }
  finally { loading.value = false; }
}
async function action(kind: "save" | "test" | "clear") {
  if (!config.value || busy.value) return;
  busy.value = true; error.value = ""; status.value = "";
  try {
    if (kind === "save") {
      const keys = removeKey.value ? { [config.value.provider]: "" } : key.value ? { [config.value.provider]: key.value } : undefined;
      await translationSaveConfig(config.value, keys);
      key.value = ""; removeKey.value = false;
      config.value = await translationConfig();
      status.value = m.value.saved;
    } else if (kind === "test") {
      const result = await translationTest(config.value.provider);
      status.value = `${m.value.testOk} · ${result.elapsed_ms} ms · ${result.text}`;
    } else { await translationClearCache(); status.value = m.value.cleared; }
  } catch (e) { error.value = errorMessage(e); }
  finally { busy.value = false; }
}
function chooseProvider(value: string) { if (config.value) config.value.provider = value; key.value = ""; removeKey.value = false; status.value = ""; }
onMounted(load);
</script>

<template>
  <section class="translation-settings" :aria-busy="busy || loading">
    <h2>{{ m.title }}</h2><p class="hint">{{ m.description }}</p>
    <CoSkeleton v-if="loading" variant="settings" :rows="4" :label="m.loading" />
    <form v-else-if="config" @submit.prevent="action('save')">
      <label>{{ m.provider }}<CoSelect :model-value="config.provider" :options="options" :disabled="busy" @update:model-value="chooseProvider" /></label>
      <p v-if="selected && !selected.available" class="hint" role="status">{{ m.unavailable }} · {{ errorMessage(new Error(selected.unavailable_reason || 'translation.unavailable')) }}</p>
      <p v-else-if="selected?.key_required" class="hint">{{ config.configured_keys.includes(config.provider) ? m.configured : m.missing }}</p>
      <label>{{ m.source }}<CoSelect v-model="config.source_language" :options="languageOptions.map(o => o.value === 'auto' ? { value: 'auto', label: m.sourceAuto } : o)" :disabled="busy" /></label>
      <p v-if="config.provider === 'mymemory' && config.source_language === 'auto'" class="hint" role="status">{{ m.sourceHint }}</p>
      <label>{{ m.target }}<CoSelect v-model="config.target_language" :options="languageOptions" :disabled="busy" /></label>
      <template v-if="providerConfig && config.provider !== 'auto'">
        <label>{{ m.endpoint }}<CoTextField v-model="providerConfig.endpoint" :disabled="busy" /></label>
        <label>{{ m.model }}<CoTextField v-model="providerConfig.model" :disabled="busy" /></label>
        <label>{{ m.region }}<CoTextField v-model="providerConfig.region" :disabled="busy" /></label>
        <label>{{ m.key }}<CoTextField v-model="key" type="password" :placeholder="m.keyHint" :disabled="busy || removeKey" /></label>
        <label class="check"><input v-model="removeKey" type="checkbox" :disabled="busy" />{{ m.removeKey }}</label>
        <label v-if="config.provider === 'deepl'" class="check"><input v-model="providerConfig.pro" type="checkbox" :disabled="busy" />{{ m.pro }}</label>
      </template>
      <label v-if="config.provider === 'auto'">{{ m.fallback }}<CoTextField :model-value="config.fallback_providers.join(', ')" :disabled="busy" @update:model-value="config.fallback_providers = $event.split(',').map(v => v.trim()).filter(Boolean)" /></label>
      <label>{{ m.cache }}<input v-model.number="config.cache_days" class="number" type="number" min="1" max="365" :disabled="busy" /><small>{{ m.cacheHint }}</small></label>
      <label>{{ m.timeout }}<input v-model.number="config.timeout_ms" class="number" type="number" min="1000" max="60000" step="1000" :disabled="busy" /></label>
      <p class="hint">{{ m.notice }}</p>
      <div class="actions"><CoButton type="submit" :disabled="busy">{{ m.save }}</CoButton><CoButton variant="secondary" :disabled="busy || !selected?.available" @click="action('test')">{{ m.test }}</CoButton><CoButton variant="secondary" :disabled="busy" @click="action('clear')">{{ m.clear }}</CoButton></div>
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
label > :deep(.co-select), label > :deep(input) { max-width: 560px; }
.hint, small { color: var(--copper-text-secondary); font-size: var(--copper-font-size-sm); overflow-wrap: anywhere; }
.check { display: flex; align-items: center; }
.number { padding: 10px 12px; color: var(--copper-text); background: var(--copper-bg); border: 0; border-radius: var(--copper-radius-control); font: inherit; max-width: 180px; }
.actions { display: flex; gap: 10px; flex-wrap: wrap; }
.error { color: var(--copper-danger); overflow-wrap: anywhere; }
</style>
