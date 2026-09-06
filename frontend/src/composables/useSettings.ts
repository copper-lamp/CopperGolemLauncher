// 设置状态：全量快照 + 类型化读写 + 订阅外部变更。
//
// 变更经 `settingsSet` / `settingsSetMany` 写回内核并广播 `settings.changed`；
// 本模块同时订阅该事件，把其它来源（模块、其它窗口）的变更同步进本地状态。

import { ref } from "vue";

import { settingsAll, settingsSet, settingsSetMany } from "../api/settings";
import { onSettingsChanged } from "../events";
import type { JsonValue } from "../api/types";
import { showToast } from "./useToast";

const snapshot = ref<Record<string, JsonValue>>({});
const loaded = ref(false);

/** 初始化：拉取全量快照并订阅变更。 */
export async function initSettings(): Promise<void> {
  if (loaded.value) return;
  loaded.value = true;
  try {
    snapshot.value = await settingsAll();
  } catch {
    // 内核未就绪时为空快照。
  }
  await onSettingsChanged((changed) => {
    Object.assign(snapshot.value, changed);
  });
}

function get<T>(key: string, fallback: T): T {
  const value = snapshot.value[key];
  if (value === undefined || value === null) return fallback;
  return value as T;
}

async function set(key: string, value: JsonValue): Promise<boolean> {
  try {
    await settingsSet(key, value);
    snapshot.value[key] = value;
    return true;
  } catch (e) {
    showToast(String(e), "error");
    return false;
  }
}

async function setMany(entries: Record<string, JsonValue>): Promise<boolean> {
  try {
    await settingsSetMany(entries);
    Object.assign(snapshot.value, entries);
    return true;
  } catch (e) {
    showToast(String(e), "error");
    return false;
  }
}

export function useSettings() {
  return {
    snapshot,
    loaded,
    get,
    set,
    setMany,
  };
}
