// 更新状态单例：状态快照 + 下载进度 + 面板开合 + 动作。
//
// 为什么做成全局单例而不是组件内状态：标题栏的更新按钮、更新弹窗、设置页的
// 「关于」区块、下载完成后的 Toast 提示是**四处互不相识的消费者**，却必须看到
// 同一份状态。四份各自订阅事件只会产生互相不同步的副本。
//
// 三条产品规则（与后端 `services::updater` 的静默检查配套）：
// 1. 启动期的检查由内核在后台发起，**失败与过程都不经前端**，前端只在拿到
//    「有新版」时才出现任何界面；
// 2. 一旦确认有可自动安装的正式版，**自动投递下载** —— 用户不需要为一次
//    必然要装的更新多点一次；
// 3. 下载完成才向用户要决策：弹窗问「是否现在更新并重启」。
//
// 进度来源：直接订阅下载引擎的 `download-progress` / `download-status` 事件流并
// 只取自己那一条任务，**不做轮询**。

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
/** 已自动投递过的版本号：同一个版本不重复投递（下载本身幂等，但会重复弹进度）。 */
let autoDownloadedFor: string | null = null;

const phase = computed(() => status.value?.phase ?? "idle");

/** 有新版本（尚未开始下载）。 */
const hasUpdate = computed(
  () => phase.value === "available" && status.value?.latest != null,
);

/** 下载中。 */
const downloading = computed(() => phase.value === "downloading");

/** 更新包已就绪，等待用户决定是否重启。 */
const ready = computed(() => phase.value === "downloaded");

/** 失败（检查失败或下载失败）。 */
const failed = computed(() => phase.value === "failed");

/** 标题栏按钮是否需要出现。 */
const visible = computed(
  () => hasUpdate.value || downloading.value || ready.value || failed.value,
);

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

/** 发行说明是否非空。 */
const hasNotes = computed(() => (latest.value?.notes ?? "").trim().length > 0);

function describeError(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

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

/** 阶段离开下载态时清进度，避免界面上残留一条走满的进度条。 */
function clearProgressWhenIdle(): void {
  if (!downloading.value) progress.value = null;
}

/**
 * 自动投递下载。
 *
 * 规则 2 的落点。三个前置条件缺一不可：
 * - 相位为 `available`（后端已确认有新版且选中了本平台产物）；
 * - 形态可自动安装（`manual` 只能提示，不能自动跑）；
 * - 不是已经投过的同一个版本（避免状态回放导致重复投递）。
 */
function maybeAutoDownload(): void {
  if (!hasUpdate.value || !autoInstallable.value) return;
  const version = latest.value?.version;
  if (!version || version === autoDownloadedFor) return;
  autoDownloadedFor = version;
  void download();
}

/** 打开更新弹窗。 */
function openPanel(): void {
  panelOpen.value = true;
}

/** 关闭更新弹窗（下载继续，不中断）。 */
function closePanel(): void {
  panelOpen.value = false;
}

/** 手动检查更新（设置页入口）。失败时后端原文交给调用方展示。 */
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
  } catch (e) {
    // 自动投递失败时不能让「有更新」永远停在原地等一个没人点的按钮：
    // 退回 `available` 相位并记下原因，用户点按钮即可重试。
    showToast(describeError(e), "error", 5000);
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

/**
 * 初始化：拉一次状态快照并订阅事件流。
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
    maybeAutoDownload();
  });
  await onDownload("progress", applyTask);
  await onDownload("status", applyTask);
  await onDownload("created", applyTask);

  await onUpdateReady(({ version }) => {
    const { t } = useI18n();
    // 下载完成是「自动下载」这条规则的终点：此刻才第一次向用户要决策。
    // Toast 承载入口（用户可能已经切到别的页面），按钮展开承载状态（余光可见）。
    showActionToast(t("update.ready_toast", { version }), {
      kind: "success",
      durationMs: 12_000,
      action: { label: t("update.install_now"), onClick: () => void install() },
    });
  });
}

/**
 * 预览注入：仅供 `preview-update.html`（视觉确认用，不参与构建产物）写入状态。
 *
 * 存在的唯一理由：按钮与弹窗的真实数据源是内核 IPC，浏览器里拿不到，
 * 就没法确认视觉效果。生产路径**不会**调用它。
 */
export function injectPreviewState(state: {
  status?: UpdateStatus | null;
  progress?: UpdateProgress | null;
  panelOpen?: boolean;
}): void {
  if ("status" in state) status.value = state.status ?? null;
  if ("progress" in state) progress.value = state.progress ?? null;
  if (state.panelOpen !== undefined) panelOpen.value = state.panelOpen;
}

export function useUpdate() {
  return {
    status: readonly(status),
    phase,
    hasUpdate,
    downloading,
    ready,
    failed,
    visible,
    latest,
    error,
    errorKind,
    autoInstallable,
    versionDelta,
    hasNotes,
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