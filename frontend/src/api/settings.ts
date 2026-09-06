// 设置 API：KV 读 / 写 / 批量写 / 全量快照。

import type { JsonValue } from "./types";
import { call } from "./core";

/** 全量设置快照（含默认值，点分键）。 */
export function settingsAll(): Promise<Record<string, JsonValue>> {
  return call<Record<string, JsonValue>>("settings_all");
}

/** 读取单个设置（不存在返回 null）。 */
export function settingsGet(key: string): Promise<JsonValue | null> {
  return call<JsonValue | null>("settings_get", { key });
}

/** 写入单个设置并广播 `settings.changed`。 */
export function settingsSet(key: string, value: JsonValue): Promise<void> {
  return call<void>("settings_set", { key, value });
}

/** 批量写入并广播一次 `settings.changed`。 */
export function settingsSetMany(entries: Record<string, JsonValue>): Promise<void> {
  return call<void>("settings_set_many", { entries });
}
