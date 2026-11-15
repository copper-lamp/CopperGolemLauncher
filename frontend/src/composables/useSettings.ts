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

const ANDROID_SAFE_COMPENSATION_KEY = "ui.androidSafeCompensation";
const ANDROID_SAFE_COMPENSATION_DEFAULT = 20;

function clampAndroidSafeCompensation(value: unknown): number {
  const numeric = typeof value === "number" ? value : Number(value);
  return Number.isFinite(numeric)
    ? Math.min(20, Math.max(0, Math.round(numeric)))
    : ANDROID_SAFE_COMPENSATION_DEFAULT;
}

function applyAndroidSafeCompensation(value: unknown): void {
  if (typeof document === "undefined") return;
  document.documentElement.style.setProperty(
    "--copper-android-safe-compensation",
    `${clampAndroidSafeCompensation(value)}px`,
  );
}

/** 初始化：拉取全量快照并订阅变更。 */
export async function initSettings(): Promise<void> {
  if (loaded.value) return;
  loaded.value = true;
  try {
    snapshot.value = await settingsAll();
    applyAndroidSafeCompensation(snapshot.value[ANDROID_SAFE_COMPENSATION_KEY]);
  } catch {
    // 内核未就绪时为空快照。
  }
  await onSettingsChanged((changed) => {
    Object.assign(snapshot.value, changed);
    if (ANDROID_SAFE_COMPENSATION_KEY in changed) {
      applyAndroidSafeCompensation(changed[ANDROID_SAFE_COMPENSATION_KEY]);
    }
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
    if (key === ANDROID_SAFE_COMPENSATION_KEY) applyAndroidSafeCompensation(value);
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

export { ANDROID_SAFE_COMPENSATION_KEY, ANDROID_SAFE_COMPENSATION_DEFAULT, clampAndroidSafeCompensation };

export function useSettings() {
  return {
    snapshot,
    loaded,
    get,
    set,
    setMany,
  };
}
