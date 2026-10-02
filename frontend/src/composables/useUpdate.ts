// 更新状态单例：状态快照 + 下载进度 + 面板开合 + 动作。
//
// 为什么做成全局单例而不是组件内状态：标题栏的更新按钮、更新面板、设置页的
// 「关于」区块、下载完成后的 Toast 提示是**四处互不相识的消费者**，却必须看到
// 同一份状态。四份各自订阅事件只会产生互相不同步的副本。
//
// 进度来源：直接订阅下载引擎的 `download-progress` / `download-status` 事件流并
// 只取自己那一条任务，**不做轮询**（旧实现每 300ms 问一次内核，纯属浪费）。

import { computed, readonly, ref } from "vue";

import {
  isAutoInstallable,
  updaterCancel,
  updaterCheck,
  updaterDownload,
  updaterInstall,
  updaterStatus,
  type UpdateInfo,
  type UpdateStatus,
} from "../api/updater";
import type { DownloadTask } from "../api/download";
import { onDownload, onUpdateReady, onUpdateStatus } from "../events";
import { showActionToast, showToast } from "./useToast";
import { useI18n } from "../i18n";

/** 更新包下载进度（由下载引擎事件推导）。 */
export interface UpdateProgress {
  /** 0~1；总量未知时为 `null`（走不确定态进度条）。 */
  ratio: number | null;
  downloaded: number;
  total: number;
  speed: number;
  /** 预计剩余秒数；速度或总量缺失时为 `null`。 */
  etaSeconds: number | null;
}

const status = ref<UpdateStatus | null>(null);
const progress = ref<UpdateProgress | null>(null);
const panelOpen = ref(false);
const busy = ref(false);
let initialized = false;

/** 当前阶段。 */
const phase = computed(() => status.value?.phase ?? "idle");

/** 有新版本（尚未开始下载）。 */
const hasUpdate = computed(
  () => phase.value === "available" && status.value?.latest != null,
);

/** 标题栏按钮是否需要出现：`available` / `downloading` / `downloaded` / `failed`。 */
const visible = computed(
  () =>
    hasUpdate.value ||
    phase.value === "downloading" ||
    phase.value === "downloaded" ||
    phase.value === "failed",
);

/** 是否正在下载。 */
const downloading = computed(() => phase.value === "downloading");

/** 更新包是否已就绪、可安装重启。 */
const ready = computed(() => phase.value === "downloaded");

/** 最新版本信息。 */
const latest = computed<UpdateInfo | null>(() => status.value?.latest ?? null);

/** 失败原因（后端原文，可直接展示）。 */
const error = computed(() => status.value?.error ?? null);

/** 失败来源。 */
const errorKind = computed(() => status.value?.error_kind ?? null);

/** 更新包形态是否可自动安装。 */
const autoInstallable = computed(() => isAutoInstallable(latest.value?.kind));

/** 是否需要展示当前版本 → 新版本的版本对照。 */
const versionDelta = computed(() => {
  const current = status.value?.current_version;
  const next = latest.value?.version;
  if (!current || !next || current === next) return null;
  return { current, next };
});

/** 由下载任务快照刷新进度。 */
function applyTask(task: DownloadTask): void {
  const id = status.value?.download_task_id;
  if (id == null || task.id !== id) return;
  const total = task.total_bytes;
  const ratio = total > 0 ? Math.min(1, task.downloaded_bytes / total) : null;
  const speed = task.speed_bytes_per_sec;
  const etaSeconds =
    ratio != null && speed > 0 && ratio < 1
      ? Math.max(0, Math.round((total - task.downloaded_bytes) / speed))
      : null;
  progress.value = {
    ratio,
    downloaded: task.downloaded_bytes,
    total,
    speed,
    etaSeconds,
  };
}

/** 阶段离开下载态时清进度，避免面板上残留一条走满的进度条。 */
function clearProgressWhenIdle(): void {
  if (!downloading.value) progress.value = null;
}

/** 打开更新面板。 */
function openPanel(): void {
  panelOpen.value = true;
}

/** 关闭更新面板（下载继续，不中断）。 */
function closePanel(): void {
  panelOpen.value = false;
}

/** 手动检查更新。失败时把后端原文交给调用方展示。 */
async function check(): Promise<UpdateStatus | null> {
  busy.value = true;
  try {
    const next = await updaterCheck();
    status.value = next;
    return next;
  } finally {
    busy.value = false;
  }
}

/** 把更新包投进下载队列。 */
async function download(): Promise<void> {
  busy.value = true;
  try {
    await updaterDownload();
  } finally {
    busy.value = false;
  }
}

/** 取消下载（保留断点）。 */
async function cancel(): Promise<void> {
  busy.value = true;
  try {
    await updaterCancel();
  } finally {
    busy.value = false;
  }
}

/** 执行替换并重启。成功即进程退出，不会返回。 */
async function install(): Promise<void> {
  busy.value = true;
  try {
    await updaterInstall();
  } catch (e) {
    // 安装失败是唯一会让应用停在旧版本却看不出原因的路径，必须弹出来。
    showToast(describeError(e), "error", 6000);
  } finally {
    busy.value = false;
  }
}

function describeError(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

/**
 * 初始化：拉一次状态快照并订阅三条事件流。
 *
 * 不在此处发起检查 —— 启动期的自动检查由内核在后台静默执行（失败不打扰用户），
 * 前端只负责把结果呈现出来。设置页的「检查更新」按钮才走 `check()`。
 */
export async function initUpdate(): Promise<void> {
  if (initialized) return;
  initialized = true;
  try {
    status.value = await updaterStatus();
  } catch {
    // 内核未就绪时保持 null，界面按「无更新」呈现。
  }

  await onUpdateStatus((next) => {
    status.value = next;
    clearProgressWhenIdle();
  });
  await onDownload("progress", applyTask);
  await onDownload("status", applyTask);
  await onDownload("created", applyTask);

  await onUpdateReady(({ version }) => {
    const { t } = useI18n();
    // 弹「立即重启」：用户可能已经切到别的页面去看下载进度了，
    // 只靠标题栏按钮提醒会被错过。
    showActionToast(t("update.ready_toast", { version }), {
      kind: "success",
      durationMs: 12_000,
      action: { label: t("update.install_now"), onClick: () => void install() },
    });
  });
}

export function useUpdate() {
  return {
    status: readonly(status),
    phase,
    hasUpdate,
    visible,
    downloading,
    ready,
    latest,
    error,
    errorKind,
    autoInstallable,
    versionDelta,
    progress: readonly(progress),
    panelOpen: readonly(panelOpen),
    busy: readonly(busy),
    openPanel,
    closePanel,
    check,
    download,
    cancel,
    install,
  };
}