// i18n 运行时：以本地语言包同步启动，随后用内核 catalog（含模块语言包）覆盖。
//
// 回退链与后端一致：目标语言 → en-US → 键名。`t` 支持 `{name}` 插值。

import { ref } from "vue";

import { i18nCatalog, i18nCurrentLocale, i18nSetLocale } from "../api/i18n";
import type { JsonValue } from "../api/types";

import zhCN from "../locales/zh-CN.json";
import enUS from "../locales/en-US.json";

// 模块语言包（与后端 `I18nService` 的 `module.<id>.*` 命名空间一致）：
// 内核 catalog 不可用（启动早期 / 浏览器调试）时前端本地兜底，
// 文件仍为单一数据源 —— 后端经 include_str 注册同一批文件。
import homeZhCN from "../modules/home/locales/zh-CN.json";
import homeEnUS from "../modules/home/locales/en-US.json";
import contentDownloadZhCN from "../modules/content-download/locales/zh-CN.json";
import contentDownloadEnUS from "../modules/content-download/locales/en-US.json";

const FALLBACK_LOCALE = "en-US";

const MODULE_LOCALES: Record<string, Record<string, JsonValue>> = {
  home: { "zh-CN": homeZhCN, "en-US": homeEnUS },
  "content-download": { "zh-CN": contentDownloadZhCN, "en-US": contentDownloadEnUS },
};

function withModulePacks(
  locale: string,
  base: Record<string, JsonValue>,
): Record<string, JsonValue> {
  const packs: Record<string, JsonValue> = {};
  for (const [moduleId, packByLocale] of Object.entries(MODULE_LOCALES)) {
    packs[moduleId] = packByLocale[locale] ?? packByLocale[FALLBACK_LOCALE];
  }
  return { ...base, module: packs };
}

const LOCAL_CATALOGS: Record<string, Record<string, JsonValue>> = {
  "zh-CN": withModulePacks("zh-CN", zhCN as unknown as Record<string, JsonValue>),
  "en-US": withModulePacks("en-US", enUS as unknown as Record<string, JsonValue>),
};

const currentLocale = ref<string>(FALLBACK_LOCALE);
const catalog = ref<Record<string, JsonValue>>(
  LOCAL_CATALOGS[FALLBACK_LOCALE],
);

/** 当前语言（响应式）。 */
export function useLocaleRef() {
  return currentLocale;
}

/** 初始化：读取当前语言并加载目录。 */
export async function initI18n(): Promise<void> {
  try {
    const locale = await i18nCurrentLocale();
    currentLocale.value = locale;
  } catch {
    // 内核未就绪时保持默认语言。
  }
  await loadCatalog(currentLocale.value);
}

/** 加载某语言目录（本地兜底 + 内核覆盖）。 */
async function loadCatalog(locale: string): Promise<void> {
  const merged: Record<string, JsonValue> = {
    ...(LOCAL_CATALOGS[locale] ?? LOCAL_CATALOGS[FALLBACK_LOCALE]),
  };
  try {
    const remote = await i18nCatalog(locale);
    Object.assign(merged, remote);
  } catch {
    // 内核不可用时用本地语言包。
  }
  catalog.value = merged;
}

/** 切换语言并持久化。 */
export async function setLocale(locale: string): Promise<void> {
  currentLocale.value = locale;
  await loadCatalog(locale);
  try {
    await i18nSetLocale(locale);
  } catch {
    // 持久化失败不阻塞界面切换。
  }
}

/** 支持的基准语言。 */
export function supportedLocales(): string[] {
  return Object.keys(LOCAL_CATALOGS);
}

/** 点分路径查询嵌套对象。 */
function lookup(root: Record<string, JsonValue>, key: string): JsonValue | undefined {
  let current: JsonValue | undefined = root;
  for (const part of key.split(".")) {
    if (current === null || typeof current !== "object" || Array.isArray(current)) {
      return undefined;
    }
    current = (current as Record<string, JsonValue>)[part];
  }
  return current;
}

function rawText(key: string): string | undefined {
  const direct = lookup(catalog.value, key);
  if (typeof direct === "string") return direct;
  if (currentLocale.value !== FALLBACK_LOCALE) {
    const fallback = lookup(LOCAL_CATALOGS[FALLBACK_LOCALE], key);
    if (typeof fallback === "string") return fallback;
  }
  return undefined;
}

/** 翻译：`t("settings.general.language")`，支持 `{param}` 插值。 */
export function t(key: string, params?: Record<string, string | number>): string {
  const text = rawText(key) ?? key;
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in params ? String(params[name]) : match,
  );
}

/** 可组合式 i18n。 */
export function useI18n() {
  return { locale: currentLocale, t, setLocale, supportedLocales };
}
