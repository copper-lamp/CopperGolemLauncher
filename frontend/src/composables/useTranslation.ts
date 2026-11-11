import { onScopeDispose, ref, watch } from "vue";
import { useLocaleRef } from "../i18n";
import { translationCancel, translationConfig, translationTranslate } from "../services/translation";
import { useTranslationError } from "../services/translation-messages";

/** Each page owns its request and generation. Cancellation never changes shared source data. */
export function useTranslation() {
  const busy = ref(false);
  const translated = ref(false);
  const error = ref("");
  const results = ref<string[]>([]);
  const locale = useLocaleRef();
  const errorMessage = useTranslationError();
  let generation = 0;
  let requestId: string | null = null;
  function reset() {
    ++generation;
    const pending = requestId;
    requestId = null;
    if (pending) void translationCancel(pending).catch(() => undefined);
    busy.value = false;
    translated.value = false;
    results.value = [];
    error.value = "";
  }
  async function run(texts: string[]) {
    reset();
    const current = generation;
    busy.value = true;
    try {
      const config = await translationConfig();
      if (current !== generation) return;
      const target = !config.target_language || config.target_language === "auto" || config.target_language === "interface"
        ? locale.value : config.target_language;
      const output: string[] = [];
      for (let offset = 0; offset < texts.length; offset += 128) {
        if (current !== generation) return;
        const id = `content-${crypto.randomUUID()}`;
        requestId = id;
        const batch = texts.slice(offset, offset + 128);
        const result = await translationTranslate(id, batch, target, config.source_language || "auto", config.generation);
        if (current !== generation) return;
        if (result.generation !== config.generation) throw new Error("translation.configuration_changed");
        if (result.texts.length !== batch.length) throw new Error("translation.invalid_response");
        output.push(...result.texts);
      }
      results.value = output;
      translated.value = true;
    } catch (cause) {
      if (current === generation) error.value = errorMessage(cause);
    } finally {
      if (current === generation) { busy.value = false; requestId = null; }
    }
  }
  watch(locale, reset);
  onScopeDispose(reset);
  return { busy, translated, error, results, run, reset };
}
