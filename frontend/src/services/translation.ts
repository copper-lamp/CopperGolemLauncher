import { invoke } from "@tauri-apps/api/core";
import { waitForIpcBridge } from "../boot";

export class TranslationError extends Error {
  constructor(readonly code: string, readonly retryable: boolean) { super(code); }
}
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!await waitForIpcBridge(2000)) throw new TranslationError("translation.ipc_unavailable", true);
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([invoke<T>(command, args), new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new TranslationError("translation.timeout", true)), command === "translation_translate" ? 310000 : 65000);
    })]);
  } catch (cause) {
    if (cause instanceof TranslationError) throw cause;
    if (typeof cause === "object" && cause && "code" in cause && typeof cause.code === "string" && /^translation\.[a-z_0-9]+$/.test(cause.code)) {
      throw new TranslationError(cause.code, "retryable" in cause && cause.retryable === true);
    }
    throw new TranslationError("translation.unavailable", true);
  } finally { if (timer) clearTimeout(timer); }
}

export interface TranslationProvider {
  id: string; name: string; endpoint: string; key_required: boolean;
  available: boolean; unavailable_reason: string | null; max_bytes: number;
  builtin_ready?: boolean; default_model?: string; supports_auto_source?: boolean;
}
export interface TranslationProviderConfig { endpoint: string; model: string; region: string; pro: boolean }
export interface TranslationConfig {
  provider: string; source_language: string; target_language: string; cache_days: number; timeout_ms: number;
  fallback_providers: string[]; providers: Record<string, TranslationProviderConfig>;
  configured_keys: string[]; generation: number;
}
export interface TranslationResult {
  request_id: string; texts: string[]; provider: string; generation: number; cached: boolean;
}
export const translationProviders = () => call<TranslationProvider[]>("translation_providers");
export const translationConfig = () => call<TranslationConfig>("translation_config");
export const translationSaveConfig = (config: TranslationConfig, keys?: Record<string, string>) =>
  call<void>("translation_save_config", { config, keys });
export const translationTest = (provider: string) =>
  call<{ provider: string; text: string; elapsed_ms: number }>("translation_test", { provider });
export const translationCancel = (requestId: string) => call<boolean>("translation_cancel", { requestId });
export const translationClearCache = () => call<number>("translation_clear_cache");
export const translationTranslate = (requestId: string, texts: string[], targetLanguage: string, sourceLanguage = "auto", expectedGeneration?: number) =>
  call<TranslationResult>("translation_translate", { request: {
    request_id: requestId, texts, source_language: sourceLanguage, target_language: targetLanguage, expected_generation: expectedGeneration,
  } });

/** Operate on already-sanitized HTML. Only text content is ever replaced. */
export function translationDocument(html: string) {
  const doc = new DOMParser().parseFromString(html, "text/html");
  const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT);
  const nodes: Array<{ node: Text; parts: string[]; indices: number[] }> = [];
  const texts: string[] = [];
  let current: Node | null;
  while ((current = walker.nextNode())) {
    if (current.parentElement?.closest("pre,code,kbd,samp,var,script,style,svg,math")) continue;
    const parts = (current.textContent ?? "").split(/(https?:\/\/[^\s<>]+|mailto:[^\s<>]+|\{\{[^{}]*\}\}|\$\{[^{}]*\}|\{[\w.-]+\}|%[\w]+%)/g);
    const indices = parts.map((part, i) => {
      if (i % 2 !== 0 || !part.trim()) return -1;
      texts.push(part); return texts.length - 1;
    });
    if (indices.some((i) => i >= 0)) nodes.push({ node: current as Text, parts, indices });
  }
  return {
    texts,
    render(translated: string[]) {
      if (translated.length !== texts.length) throw new Error("Translation segment count mismatch");
      nodes.forEach(({ node, parts, indices }) => {
        node.textContent = parts.map((part, i) => indices[i] < 0 ? part : translated[indices[i]]).join("");
      });
      return doc.body.innerHTML;
    },
  };
}
