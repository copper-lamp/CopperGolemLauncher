import { onScopeDispose, ref, watch } from "vue";
import { useLocaleRef } from "../i18n";
import { translationCancel, translationConfig, translationTranslate } from "../services/translation";
import { useTranslationError } from "../services/translation-messages";

/** Each page owns its operation; cancellation also stops late provider sampling. */
export function useTranslation() {
  const busy = ref(false);
  const translated = ref(false);
  const error = ref("");
  const results = ref<string[]>([]);
  const locale = useLocaleRef();
  const errorMessage = useTranslationError();
  let generation = 0;
  const requestIds = new Set<string>();
  function cancelRequests() {
    for (const id of requestIds) void translationCancel(id).catch(() => undefined);
    requestIds.clear();
  }
  function reset() {
    ++generation;
    cancelRequests();
    busy.value = false;
    translated.value = false;
    results.value = [];
    error.value = "";
  }
  async function run(texts: string[], publish?: (values: string[]) => void): Promise<string[] | null> {
    reset();
    const current = generation;
    const operationId = `content-operation-${crypto.randomUUID()}`;
    busy.value = true;
    try {
      const config = await translationConfig();
      if (current !== generation) return null;
      const target = !config.target_language || config.target_language === "auto" || config.target_language === "interface"
        ? locale.value : config.target_language;
      // Keep the exact source slots, but send each nonempty text only once.
      const unique = [...new Set(texts.filter(text => text.trim()))];
      const positions = new Map(unique.map((text, index) => [text, index]));
      const output = [...unique];
      const values = () => texts.map(text => {
        const index = positions.get(text);
        return index === undefined ? text : output[index];
      });
      const batches: Array<{ offset: number; texts: string[] }> = [];
      for (let offset = 0; offset < unique.length;) {
        const start = offset;
        let chars = 0;
        while (offset < unique.length && offset - start < 8) {
          const length = unique[offset].length;
          if (offset > start && chars + length > 6000) break;
          chars += length;
          ++offset;
        }
        batches.push({ offset: start, texts: unique.slice(start, offset) });
      }
      let stopped = false;
      const send = async (batch: { offset: number; texts: string[] }, final: boolean) => {
        if (stopped || current !== generation) return;
        const id = `content-${crypto.randomUUID()}`;
        requestIds.add(id);
        try {
          const result = await translationTranslate(id, batch.texts, target, config.source_language || "auto", config.generation, operationId, final);
          if (stopped || current !== generation) return;
          if (result.generation !== config.generation) throw new Error("translation.configuration_changed");
          if (result.texts.length !== batch.texts.length) throw new Error("translation.invalid_response");
          output.splice(batch.offset, batch.texts.length, ...result.texts);
          const partial = values();
          publish?.(partial);
          results.value = partial;
          translated.value = true;
        } catch (cause) { stopped = true; throw cause; }
      };
      // Two bounded workers; seal the operation only after all preceding batches settled.
      let cursor = 0;
      const worker = async () => {
        while (!stopped && current === generation && cursor < batches.length - 1) {
          const batch = batches[cursor++];
          await send(batch, false);
        }
      };
      await Promise.all([worker(), worker()]);
      if (stopped || current !== generation) return null;
      if (batches.length) await send(batches[batches.length - 1], true);
      if (current !== generation) return null;
      const completed = values();
      publish?.(completed);
      results.value = completed;
      translated.value = true;
      return completed;
    } catch (cause) {
      if (current === generation) {
        cancelRequests();
        results.value = [];
        translated.value = false;
        error.value = errorMessage(cause);
      }
      return null;
    } finally {
      if (current === generation) busy.value = false;
    }
  }
  watch(locale, reset);
  onScopeDispose(reset);
  return { busy, translated, error, results, run, reset };
}
