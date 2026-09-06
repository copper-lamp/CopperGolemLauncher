// i18n API：语言目录、支持语言、当前语言、切换语言。

import type { JsonValue } from "./types";
import { call } from "./core";

/** 某语言完整目录（基准 + 模块语言包）。 */
export function i18nCatalog(locale: string): Promise<Record<string, JsonValue>> {
  return call<Record<string, JsonValue>>("i18n_catalog", { locale });
}

/** 支持的基准语言列表。 */
export function i18nSupportedLocales(): Promise<string[]> {
  return call<string[]>("i18n_supported_locales");
}

/** 当前语言。 */
export function i18nCurrentLocale(): Promise<string> {
  return call<string>("i18n_current_locale");
}

/** 切换语言并持久化。 */
export function i18nSetLocale(locale: string): Promise<void> {
  return call<void>("i18n_set_locale", { locale });
}
