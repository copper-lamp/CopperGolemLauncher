// 下载列表状态：初始化拉取快照 + 订阅引擎事件增量更新。
//
// 供下载悬浮窗与下载页共用；模块投递任务后此处自动出现。

import { computed, readonly, ref } from "vue";

import {
  downloadTasks,
  type DownloadTask,
  downloadPause,
  downloadResume,
  downloadCancel,
  downloadRetry,
  downloadRemove,
  downloadPauseAll,
  downloadResumeAll,
  downloadConcurrency,
  downloadSetConcurrency,
} from "../api/download";
import { onDownload } from "../events";
import { showToast } from "./useToast";

const coreTasks = ref<DownloadTask[]>([]);
/**
 * 前端合成的“复用本地整包”安装条目。
 *
 * 复用整包时后端直接进安装、不创建下载任务，下载引擎不会发 created/progress。
 * 这里补一条 installing 行，让下载列表仍出现该项目并播放安装动画，只是跳过下载。
 */
const installOverlays = ref<DownloadTask[]>([]);
const installTimers = new Map<number, ReturnType<typeof setInterval>>();
let overlaySeq = 1;

const tasks = computed<DownloadTask[]>(() => [
  ...coreTasks.value,
  ...installOverlays.value,
]);
const concurrency = ref(3);
const initialized = ref(false);

function upsert(task: DownloadTask) {
  const index = coreTasks.value.findIndex((t) => t.id === task.id);
  if (index >= 0) {
    coreTasks.value[index] = task;
  } else {
    coreTasks.value.push(task);
  }
}

/** 安装任务开始（复用整包）时补一条可见条目。 */
export function announceInstall(instance: string): void {
  const id = -overlaySeq++;
  const task: DownloadTask = {
    id,
    filename: instance,
    url: `install://${instance}`,
    dest: "",
    total_bytes: 1,
    downloaded_bytes: 1,
    speed_bytes_per_sec: 0,
    status: "installing",
    error: null,
    retry_count: 0,
    created_at_ms: Date.now(),
    phase_progress: 0.02,
    stage: "download.stage.install_reusing",
    stage_detail: instance,
  };
  installOverlays.value = [...installOverlays.value, task];
  const timer = setInterval(() => {
    const current = installOverlays.value.find((item) => item.id === id);
    if (!current) {
      clearInterval(timer);
      return;
    }
    // 后端没有复用路径的逐帧进度；用一个缓慢逼近 95% 的动画表达“正在安装”，
    // 真正的完成/失败由 game-download.installed / failed 事件收尾。
    current.phase_progress = Math.min(0.95, (current.phase_progress ?? 0) + 0.02);
    installOverlays.value = [...installOverlays.value];
  }, 600);
  installTimers.set(id, timer);
}

function dropInstallOverlay(id: number): void {
  const timer = installTimers.get(id);
  if (timer !== undefined) clearInterval(timer);
  installTimers.delete(id);
  installOverlays.value = installOverlays.value.filter((item) => item.id !== id);
}

/** 安装完成：移除对应实例的合成条目。 */
export function finishInstallOverlay(instance: string): void {
  for (const item of [...installOverlays.value]) {
    if (item.filename === instance) dropInstallOverlay(item.id);
  }
}

/** 安装失败/取消：清掉所有合成条目，避免永远停在“安装中”。 */
export function clearInstallOverlays(): void {
  for (const item of [...installOverlays.value]) dropInstallOverlay(item.id);
}

/** 拉取全量快照（控制类操作后调用）。 */
async function refresh(): Promise<void> {
  try {
    coreTasks.value = await downloadTasks();
  } catch {
    // 内核未就绪时保留现有列表。
  }
}

/** 初始化：拉取一次全量快照与并发数，并订阅增量事件。 */
export async function initDownloads(): Promise<void> {
  if (initialized.value) return;
  initialized.value = true;
  try {
    coreTasks.value = await downloadTasks();
  } catch {
    // 内核未就绪时为空列表。
  }
  try {
    concurrency.value = await downloadConcurrency();
  } catch {
    // 内核未就绪时保留默认值。
  }
  await Promise.all([
    onDownload("created", upsert),
    onDownload("progress", upsert),
    onDownload("status", upsert),
  ]);
}

/** 活跃任务数（下载中 + 排队中）。 */
function activeCount(): number {
  return tasks.value.filter(
    (t) =>
      t.status === "downloading" ||
      t.status === "queued" ||
      t.status === "installing",
  ).length;
}

export function useDownloads() {
  return {
    tasks: readonly(tasks),
    concurrency: readonly(concurrency),
    activeCount,
    /** 重拉全量快照（批量操作后由调用方决定何时刷新）。 */
    refresh,
    async setConcurrency(value: number) {
      try {
        concurrency.value = await downloadSetConcurrency(value);
      } catch (e) {
        showToast(String(e), "error");
      }
    },
    async pause(id: number) {
      try {
        await downloadPause(id);
      } catch (e) {
        showToast(String(e), "error");
      }
    },
    async resume(id: number) {
      try {
        await downloadResume(id);
        await refresh();
      } catch (e) {
        showToast(String(e), "error");
      }
    },
    async cancel(id: number) {
      try {
        await downloadCancel(id);
      } catch (e) {
        showToast(String(e), "error");
      }
    },
    async retry(id: number) {
      try {
        await downloadRetry(id);
        await refresh();
      } catch (e) {
        showToast(String(e), "error");
      }
    },
    async remove(id: number) {
      try {
        await downloadRemove(id);
        // 移除会同时删掉内存任务与持久化记录，而这两者都不发事件
        // （引擎的 remove 是静默的）。不重拉的话这一行会一直留在界面上，
        // 表现为「点删除没反应」。
        await refresh();
      } catch (e) {
        showToast(String(e), "error");
      }
    },
    async pauseAll() {
      try {
        await downloadPauseAll();
      } catch (e) {
        showToast(String(e), "error");
      }
    },
    async resumeAll() {
      try {
        await downloadResumeAll();
      } catch (e) {
        showToast(String(e), "error");
      }
    },
  };
}
