// 下载列表状态：初始化拉取快照 + 订阅引擎事件增量更新。
//
// 供下载悬浮窗与下载页共用；模块投递任务后此处自动出现。

import { readonly, ref } from "vue";

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

const tasks = ref<DownloadTask[]>([]);
const concurrency = ref(3);
const initialized = ref(false);

function upsert(task: DownloadTask) {
  const index = tasks.value.findIndex((t) => t.id === task.id);
  if (index >= 0) {
    tasks.value[index] = task;
  } else {
    tasks.value.push(task);
  }
}

/** 拉取全量快照（控制类操作后调用）。 */
async function refresh(): Promise<void> {
  try {
    tasks.value = await downloadTasks();
  } catch {
    // 内核未就绪时保留现有列表。
  }
}

/** 初始化：拉取一次全量快照与并发数，并订阅增量事件。 */
export async function initDownloads(): Promise<void> {
  if (initialized.value) return;
  initialized.value = true;
  try {
    tasks.value = await downloadTasks();
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
    (t) => t.status === "downloading" || t.status === "queued",
  ).length;
}

export function useDownloads() {
  return {
    tasks: readonly(tasks),
    concurrency: readonly(concurrency),
    activeCount,
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
