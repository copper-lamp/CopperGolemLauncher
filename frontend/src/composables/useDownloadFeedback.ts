import { computed, readonly, ref } from "vue";
import { contentDownloadRecords, type ContentDownloadRecord } from "../modules/content-download/api";
import { onContentInstallFinished, onContentInstallProgress, onContentRecordsChanged, onGameDownloadInstalled } from "../events";
import { useDownloads } from "./useDownloads";
import { showToast } from "./useToast";
import { useI18n } from "../i18n";
import { reportDomainFailure } from "../services/errtips";

const contentRecords = ref<ContentDownloadRecord[]>([]);
const installDetails = ref<Record<string, string>>({});
const unreadCompletedCount = ref(0);
const completionPulseSeq = ref(0);
const completed = new Set<string>();
const observed = new Map<string, string>();
let target: HTMLElement | null = null;
let visible = false;
let initialized: Promise<void> | undefined;
let generation = 0;
let baselineReady = false;
let pendingRefresh = false;
const { t } = useI18n();

function complete(key: string, name: string, toast = true) {
  if (completed.has(key)) return;
  completed.add(key);
  if (!visible) unreadCompletedCount.value++;
  completionPulseSeq.value++;
  if (toast) showToast(t("download.feedback_completed", { name }), "success");
}
async function refreshRecords(baseline = false) {
  if (!baseline && !baselineReady) { pendingRefresh = true; return; }
  const seq = ++generation;
  const rows = await contentDownloadRecords();
  if (seq !== generation) return;
  for (const row of rows) {
    const previous = observed.get(row.id);
    if (!baseline && previous !== "installed" && row.state === "installed") complete(`content:${row.id}`, row.name);
    observed.set(row.id, row.state);
  }
  contentRecords.value = rows;
}

export function initDownloadFeedback(): Promise<void> {
  return initialized ??= (async () => {
    // Register first. A baseline snapshot is silent so history never becomes new unread work.
    await Promise.all([
      onContentInstallProgress(p => {
        const row = contentRecords.value.find(r => r.id === p.id);
        if (row) { row.progress = p.progress; row.stage = p.stage; }
        if (p.stageDetail) installDetails.value[p.id] = p.stageDetail;
      }),
      onContentRecordsChanged(() => { void refreshRecords().catch(() => {}); }),
      onContentInstallFinished(({ operationId, outcome }) => {
        if (!outcome.success && !completed.has(`failure:${operationId}`)) {
          completed.add(`failure:${operationId}`);
          reportDomainFailure({ module: "content-download", operation: "lip-install", code: outcome.errorCode ?? "unknown", stage: "install", job_id: operationId,
            timeout_scope: outcome.errorCode === "ERR_LIP_DAEMON_TIMEOUT" ? "daemon_rpc" : undefined });
        }
        void refreshRecords().catch(() => {});
        void operationId;
      }),
      onGameDownloadInstalled(({ id, instance }) => complete(`game:${id}:${instance}`, instance, false)),
    ]);
    await refreshRecords(true).catch(() => {});
    baselineReady = true;
    if (pendingRefresh) { pendingRefresh = false; await refreshRecords().catch(() => {}); }
  })();
}

export function flyToDownloads(origin?: { x: number; y: number }) {
  if (!target || !origin || window.matchMedia("(prefers-reduced-motion: reduce)").matches) { completionPulseSeq.value++; return; }
  const r = target.getBoundingClientRect();
  const end = { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  const dot = document.createElement("div");
  dot.style.cssText = "position:fixed;left:0;top:0;width:10px;height:10px;border-radius:50%;background:var(--copper-accent);pointer-events:none;z-index:10000";
  document.body.append(dot);
  const ctrl = { x: (origin.x + end.x) / 2, y: Math.min(origin.y, end.y) - 120 };
  const frames = Array.from({ length: 33 }, (_, i) => {
    const a = i / 32;
    const x = (1-a)**2 * origin.x + 2*(1-a)*a*ctrl.x + a*a*end.x;
    const y = (1-a)**2 * origin.y + 2*(1-a)*a*ctrl.y + a*a*end.y;
    return { transform: `translate3d(${x}px,${y}px,0) scale(${1-.35*a})`, opacity: 1 };
  });
  const animation = dot.animate(frames, { duration: 640, easing: "ease-in" });
  animation.onfinish = () => { dot.remove(); completionPulseSeq.value++; };
  animation.oncancel = () => dot.remove();
}

export function useDownloadFeedback() {
  const downloads = useDownloads();
  return {
    contentRecords: readonly(contentRecords), installDetails: readonly(installDetails),
    unreadCompletedCount: readonly(unreadCompletedCount), completionPulseSeq: readonly(completionPulseSeq),
    activeCount: computed(() => downloads.activeCount() + contentRecords.value.filter(r => r.state === "installing").length),
    markCompletionsRead: () => { unreadCompletedCount.value = 0; },
    setDownloadCenterVisible: (value: boolean) => { visible = value; if (value) unreadCompletedCount.value = 0; },
    registerDownloadTarget: (el: HTMLElement | null) => { target = el; },
    refreshRecords: () => refreshRecords(),
  };
}
