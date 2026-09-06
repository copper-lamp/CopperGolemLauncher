// 游戏下载状态：初始化拉取清单 + 订阅引擎事件增量更新 + 模块生命周期事件。
//
// 作为模块级单例（如 `useDownloads`），供列表页与详情页共用。任务进度来自
// 内核全局 `download.progress/status` 事件（按本模块文件名 `{slug}.msixvc` 过滤），
// 安装完成 / 失败 / 取消经模块专用事件驱动清单刷新与反馈。

import { readonly, ref } from "vue";

import type { DownloadTask } from "../../api/download";
import { showToast } from "../../composables/useToast";
import {
  onDownload,
  onGameDownloadCancelled,
  onGameDownloadEnqueued,
  onGameDownloadFailed,
  onGameDownloadInstalled,
} from "../../events";
import { t } from "../../i18n";
import {
  formatBytes as fmtBytes,
  gameCancel,
  gameDetail,
  gameEnqueue,
  gameManifest,
  gameRefreshSource,
  progressRatio,
  type GameManifestView,
  type GameTaskView,
} from "./api";

/** 版本 id → 实时下载任务（来自全局事件，按文件名过滤）。 */
const liveTasks = ref<Record<string, DownloadTask>>({});
/** 版本 id → 版本任务视图（从事务查询，含解包/失败状态与错误）。 */
const taskStates = ref<Record<string, GameTaskView>>({});

const manifest = ref<GameManifestView | null>(null);
const loading = ref(false);
const initialized = ref(false);

/** 从全局任务快照反解本模块版本 id（文件名 `{slug}.msixvc`）。 */
function versionIdOf(task: DownloadTask): string | null {
  const name = task.filename ?? "";
  const base = name.endsWith(".msixvc") ? name.slice(0, -7) : null;
  return base && base.length > 0 ? base : null;
}

function upsertLive(task: DownloadTask) {
  const id = versionIdOf(task);
  if (!id) return; // 非本模块任务
  liveTasks.value = { ...liveTasks.value, [id]: task };
}

function setTaskState(state: GameTaskView) {
  taskStates.value = { ...taskStates.value, [state.version_id]: state };
}

/** 初始化：订阅一次全局下载 + 模块事件。 */
export async function initGameDownload(): Promise<void> {
  if (initialized.value) return;
  initialized.value = true;
  await Promise.all([
    onDownload("progress", upsertLive),
    onDownload("status", upsertLive),
    onGameDownloadEnqueued(({ id, taskId }) => {
      // 入队即开始回填状态；随后 progress/status 事件驱动实时进度。
      void refreshState(id);
    }),
    onGameDownloadInstalled(() => {
      showToast(t("module.game-download.toast.installed"), "success");
      void loadManifest(false);
    }),
    onGameDownloadFailed(({ error }) => {
      showToast(error || t("module.game-download.toast.failed"), "error");
      void loadManifest(false);
    }),
    onGameDownloadCancelled(() => {
      showToast(t("module.game-download.toast.cancelled"), "info");
      void loadManifest(false);
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

/** 刷新单个版本的任务状态（详情页 / 入队后回填）。 */
export async function refreshState(id: string): Promise<void> {
  try {
    const state = await gameDetail(id);
    if (state) setTaskState(state);
  } catch {
    // 忽略：无任务或查询失败不影响界面。
  }
}

/** 投递下载。 */
export async function enqueue(id: string): Promise<void> {
  try {
    const taskId = await gameEnqueue(id);
    await refreshState(id);
    if (taskId > 0) showToast(t("module.game-download.toast.enqueued"), "success");
  } catch (e) {
    showToast(String(e), "error");
  }
}

/** 取消任务。 */
export async function cancel(id: string): Promise<void> {
  try {
    await gameCancel(id);
  } catch (e) {
    showToast(String(e), "error");
  }
}

/** 强制刷新清单源。 */
export async function refreshSource(): Promise<void> {
  await loadManifest(true);
}

/** 单一版本视图（时间序最近一次任务状态优先）。 */
export function useGameDownload() {
  return {
    manifest: readonly(manifest),
    loading: readonly(loading),
    liveTasks: readonly(liveTasks),
    taskStates: readonly(taskStates),
    loadManifest,
    refreshState,
    enqueue,
    cancel,
    refreshSource,
    percentOf: (id: string): number => {
      const live = liveTasks.value[id];
      if (live) return progressRatioFromSnapshot(live);
      const state = taskStates.value[id];
      return progressRatio(state?.download ?? null);
    },
    speedOf: (id: string): string => {
      const live = liveTasks.value[id];
      return live ? fmtBytes(live.speed_bytes_per_sec) + "/s" : "";
    },
    rawOf: (id: string): string => {
      const live = liveTasks.value[id];
      if (live) return `${fmtBytes(live.downloaded_bytes)} / ${fmtBytes(live.total_bytes)}`;
      const state = taskStates.value[id];
      if (state?.download) {
        return `${fmtBytes(state.download.downloaded_bytes)} / ${fmtBytes(state.download.total_bytes)}`;
      }
      return "";
    },
  };
}

/** 从全局任务快照计算进度（0~1）。 */
function progressRatioFromSnapshot(task: DownloadTask): number {
  if (task.total_bytes <= 0) return 0;
  return Math.min(task.downloaded_bytes / task.total_bytes, 1);
}