// 下载列表状态：初始化拉取快照 + 订阅引擎事件增量更新。
//
// 供下载悬浮窗与下载页共用；模块投递任务后此处自动出现。

import { readonly, ref } from "vue";

import {
  downloadTasks,
  downloadPause,
  downloadResume,
  downloadCancel,
  downloadRetry,
  downloadRemove,
  downloadPauseAll,
  downloadResumeAll,
  type DownloadTask,
} from "../api/download";
import { onDownload } from "../events";
import { showToast } from "./useToast";

const tasks = ref<DownloadTask[]>([]);
const initialized = ref(false);

function upsert(task: DownloadTask) {
  const index = tasks.value.findIndex((t) => t.id === task.id);
  if (index >= 0) {
    tasks.value[index] = task;
  } else {
    tasks.value.push(task);
  }
}

/** 初始化：拉取一次全量快照并订阅增量事件。 */
export async function initDownloads(): Promise<void> {
  if (initialized.value) return;
  initialized.value = true;
  try {
    tasks.value = await downloadTasks();
  } catch {
    // 内核未就绪时为空列表。
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
    activeCount,
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
      } catch (e) {
        showToast(String(e), "error");
      }
    },
    async remove(id: number) {
      try {
        await downloadRemove(id);
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
