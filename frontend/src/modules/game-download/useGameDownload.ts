// 游戏下载状态：清单 + 加载器清单 + 安装投递 + 模块事件。
//
// 作为模块级单例（如 `useDownloads`），供列表页与详情页共用。
//
// **这里不跟踪下载 / 安装进度**：进度由内核下载引擎统一上报，界面统一在下载中心
// 展示（含解包与加载器安装的阶段进度）。模块再存一份实时进度只会造成两处显示不同步
// ——下载中心是任务的事实源，这里只负责「清单」与「投递」。

import { readonly, ref } from "vue";

import { showToast } from "../../composables/useToast";
import {
  onGameDownloadCancelled,
  onGameDownloadEnqueued,
  onGameDownloadFailed,
  onGameDownloadInstalled,
} from "../../events";
import { t } from "../../i18n";
import {
  gameCancel,
  gameInstall,
  gameInstanceCheck,
  gameInstanceSuggest,
  gameLoaders,
  gameManifest,
  gameRetry,
  type GameManifestView,
  type InstanceCheck,
  type LoaderOptions,
} from "./api";

const manifest = ref<GameManifestView | null>(null);
const loading = ref(false);
const initialized = ref(false);
/** 版本 id → 加载器清单（按需拉取后在本次会话内缓存）。 */
const loaderCatalogs = ref<Record<string, LoaderOptions>>({});
/** 同一版本的加载器清单并发请求去重（详情页快速切换时会出现）。 */
const loaderInflight = new Map<string, Promise<LoaderOptions | null>>();

/** 初始化：订阅模块事件（幂等）。 */
export async function initGameDownload(): Promise<void> {
  if (initialized.value) return;
  initialized.value = true;
  await Promise.all([
    onGameDownloadEnqueued(() => {
      // 入队即刷新清单：加载器徽标等派生信息可能因此变化。
      void loadManifest(false);
    }),
    onGameDownloadInstalled(({ instance }) => {
      showToast(t("module.game-download.toast.installed", { name: instance }), "success");
      void loadManifest(false);
    }),
    onGameDownloadFailed(({ error }) => {
      showToast(error || t("module.game-download.toast.failed"), "error");
    }),
    onGameDownloadCancelled(() => {
      showToast(t("module.game-download.toast.cancelled"), "info");
    }),
  ]);
}

/** 拉取清单（`refresh=true` 强制网络刷新源）。 */
export async function loadManifest(refresh = false): Promise<void> {
  loading.value = true;
  try {
    manifest.value = await gameManifest(refresh);
    if (refresh) showToast(t("module.game-download.toast.refresh_ok"), "success");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    loading.value = false;
  }
}

/**
 * 拉取某版本的加载器清单。
 *
 * 失败时返回 `null` 并提示：加载器是**可选**项，服务不可达不该阻断安装流程，
 * 下拉退化为「无可用加载器」即可。
 */
export async function loadLoaders(id: string, force = false): Promise<LoaderOptions | null> {
  if (!force && loaderCatalogs.value[id]) return loaderCatalogs.value[id];
  const existing = loaderInflight.get(id);
  if (existing) return existing;
  const request = (async () => {
    try {
      const options = await gameLoaders(id);
      loaderCatalogs.value = { ...loaderCatalogs.value, [id]: options };
      return options;
    } catch (e) {
      showToast(String(e), "error");
      return null;
    } finally {
      loaderInflight.delete(id);
    }
  })();
  loaderInflight.set(id, request);
  return request;
}

/** 推荐实例名（安装确认弹窗初值）。失败时回落到版本号本身。 */
export async function suggestInstance(id: string, fallback: string): Promise<string> {
  try {
    return await gameInstanceSuggest(id);
  } catch {
    return fallback;
  }
}

/** 实例名可用性检查；查询失败时返回 `null`（调用方按「无法确认」处理，不放行）。 */
export async function checkInstance(name: string): Promise<InstanceCheck | null> {
  try {
    return await gameInstanceCheck(name);
  } catch {
    return null;
  }
}

/**
 * 投递安装：以 `instance` 为实例名安装版本 `id`，可选加载器 `loader`。
 *
 * 返回 `true` 表示投递成功（弹窗可以关闭）。返回的任务 id 为 0 时说明整包已在本地，
 * 后端已直接开始安装——不必提示「开始下载」。
 */
export async function install(
  id: string,
  instance: string,
  loader?: string | null,
): Promise<boolean> {
  try {
    const taskId = await gameInstall(id, instance, loader);
    showToast(
      taskId > 0
        ? t("module.game-download.toast.enqueued")
        : t("module.game-download.toast.reusing"),
      "success",
    );
    await loadManifest(false);
    return true;
  } catch (e) {
    showToast(String(e), "error");
    return false;
  }
}

/** 取消某实例的安装（该版本再无待装实例时连整包下载一起放弃）。 */
export async function cancel(instance: string): Promise<void> {
  try {
    await gameCancel(instance);
  } catch (e) {
    showToast(String(e), "error");
  }
}

/** 重试某实例的安装：整包在本地时不重新下载。 */
export async function retry(instance: string): Promise<void> {
  try {
    await gameRetry(instance);
    showToast(t("module.game-download.toast.installing"), "info");
  } catch (e) {
    showToast(String(e), "error");
  }
}

/** 游戏下载模块的共享状态与操作。 */
export function useGameDownload() {
  return {
    manifest: readonly(manifest),
    loading: readonly(loading),
    loaderCatalogs: readonly(loaderCatalogs),
    loadManifest,
    loadLoaders,
    suggestInstance,
    checkInstance,
    install,
    cancel,
    retry,
  };
}
