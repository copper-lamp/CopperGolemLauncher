// 安卓 APK 导入的文件选择通道。
//
// 为什么不用 `@tauri-apps/plugin-dialog`：该插件在 Android 上只回传
// `content://` URI，既不落盘也无法被 Rust 读取（内核拿到的不是文件路径）。
// 因此这里走「深链唤起安卓宿主 → 宿主流式写入导入事务目录 → 轮询结果文件」
// 三步，结果文件由 `android_apk_pick_result` 以 take 语义取走。
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";

import { androidApkPickResult, type AndroidApkPickResult } from "./api";

/** 轮询间隔：宿主复制 200MB 级 APK 需要数秒到数十秒。 */
const POLL_INTERVAL_MS = 400;
/** 轮询上限：用户在系统选择器里可能长时间不操作，超过后放弃本次导入。 */
const POLL_TIMEOUT_MS = 180_000;

function newRequestId(): string {
  return crypto.randomUUID();
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

export interface PickedApk {
  /** 应用私有目录内的绝对路径，可直接交给 `gameImportApk`。 */
  path: string;
  displayName: string;
  iconPng: string | null;
  versionName: string | null;
}

/**
 * 唤起系统文件选择器并等待安卓宿主将文件写入导入事务目录。
 *
 * @param cancelMessage 用户取消时抛出，便于调用方静默处理。
 */
export async function pickApkViaHost(cancelMessage: string): Promise<PickedApk> {
  const requestId = newRequestId();
  const url = `coppergolem://import-apk?request_id=${encodeURIComponent(requestId)}`;
  try {
    await invoke("game_download_prepare_apk_import", { requestId });
    await openExternal(url);

    const deadline = Date.now() + POLL_TIMEOUT_MS;
    while (Date.now() < deadline) {
      await sleep(POLL_INTERVAL_MS);
      let result: AndroidApkPickResult | null = null;
      try {
        result = await androidApkPickResult(requestId);
      } catch (error) {
        throw new Error(`读取文件选择结果失败: ${String(error)}`);
      }
      if (result === null) continue;
      if (result.error) {
        throw new Error(result.error === CANCELLED_MARKER ? cancelMessage : result.error);
      }
      if (!result.path) throw new Error("文件选择未返回可用路径");
      // `displayName` 与内核的序列化键名一致（camelCase）；读 snake_case 会得到
      // undefined，随后在实例名规整处抛出一个与真实原因无关的 TypeError.
      const displayName = result.displayName || result.path.split("/").pop() || "import.apk";
      return { path: result.path, displayName, iconPng: result.iconPng ?? null, versionName: result.versionName ?? null };
    }
    throw new Error("文件选择超时");
  } catch (error) {
    await removeImportTransaction(requestId).catch(() => {});
    throw error;
  }
}

async function removeImportTransaction(requestId: string): Promise<void> {
  await invoke("game_download_discard_apk_import", { requestId }).catch(() => {});
}

const CANCELLED_MARKER = "用户取消了选择";

/**
 * 记录导入失败到内核日志。
 *
 * 准备阶段的长耗时与失败原因都只存在于安卓宿主，宿主侧已写 logcat；
 * 这里再补一条到内核日志，保证用户反馈 issue 时两边都有。
 */
export function reportAndroidImportFailure(stage: string, error: unknown): void {
  void invoke("debug_log", {
    level: "error",
    message: `android apk import failed at ${stage}: ${String(error)}`,
  }).catch(() => {});
}
