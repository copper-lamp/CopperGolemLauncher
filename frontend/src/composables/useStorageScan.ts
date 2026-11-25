// 存储用量扫描状态：先订阅事件、再拉内存快照，随后交给后端后台线程扫描。
//
// 关键约束：
// 1. 扫描全程不阻塞界面：这里所有命令都是毫秒级（读内存快照 / 投递线程）；
// 2. 扫描中不展示半成品总量：totalBytes 只在 ready 时给出，分类数值按事件逐个填充；
// 3. 事件按 scanId 过滤：上一轮扫描的尾部事件不能覆盖新一轮的结果。

import { computed, readonly, ref } from "vue";

import {
  storageClean,
  storageScanStart,
  storageUsageSnapshot,
  type StorageCategoryScan,
  type StorageScanSnapshot,
} from "../api/storage";
import {
  onStorageCategoryScanned,
  onStorageScanFailed,
  onStorageScanFinished,
  onStorageScanProgress,
  type Unlisten,
} from "../events";

const snapshot = ref<StorageScanSnapshot | null>(null);
const progress = ref<{ category: string; files: number; bytes: number } | null>(null);
const starting = ref(false);
const requestError = ref<string | null>(null);

const listeners: Unlisten[] = [];

/** 同一会话内短时间内重复进入设置页不重扫：刚扫完的结果仍然有效。 */
const RESCAN_GAP_MS = 20_000;

function apply(next: StorageScanSnapshot | null): void {
  if (!next) return;
  // 快照是当前状态，但响应可能晚于更新的事件到达，按 scanId 丢弃过期结果。
  if (snapshot.value && next.scanId < snapshot.value.scanId) return;
  snapshot.value = next;
  requestError.value = null;
}

function accept(scanId: number): boolean {
  return snapshot.value !== null && snapshot.value.scanId === scanId;
}

function patch(patchValue: Partial<StorageScanSnapshot>): void {
  if (!snapshot.value) return;
  snapshot.value = { ...snapshot.value, ...patchValue };
}

function mergeCategory(category: StorageCategoryScan): void {
  if (!snapshot.value) return;
  const categories = snapshot.value.categories.map((item) =>
    item.id === category.id ? category : item,
  );
  patch({ categories });
}

async function subscribe(): Promise<void> {
  if (listeners.length) return;
  const created = await Promise.all([
    onStorageScanProgress((payload) => {
      if (!accept(payload.scanId)) return;
      progress.value = {
        category: payload.category,
        files: payload.files,
        bytes: payload.bytes,
      };
    }),
    onStorageCategoryScanned((payload) => {
      if (!accept(payload.scanId)) return;
      mergeCategory(payload.category);
    }),
    onStorageScanFinished((payload) => {
      progress.value = null;
      apply(payload.snapshot);
    }),
    onStorageScanFailed((payload) => {
      if (!accept(payload.scanId)) return;
      progress.value = null;
      patch({ state: "failed", error: payload.error });
    }),
  ]);
  listeners.push(...created);
}

/** 启动（或复用）后台扫描；命令立即返回，结果靠事件推进。 */
async function rescan(force = true): Promise<void> {
  starting.value = true;
  try {
    apply(await storageScanStart(force));
  } catch (error) {
    requestError.value = String(error);
  } finally {
    starting.value = false;
  }
}

/** 清理后的快照同步：清理命令已在后端扣减数值，这里只取回结果，不再重扫。 */
async function refreshSnapshot(): Promise<void> {
  try {
    apply(await storageUsageSnapshot());
  } catch {
    // 内核未就绪时保留现有数据。
  }
}

/** 进入设置页时调用：并发调用合并成一次；每次进入都重新判断结果是否还新鲜。 */
let initPromise: Promise<void> | null = null;

async function init(): Promise<void> {
  if (initPromise) return initPromise;
  initPromise = (async () => {
    await subscribe();
    await refreshSnapshot();
    const current = snapshot.value;
    const fresh =
      current?.state === "ready" &&
      current.finishedAt !== null &&
      Date.now() - current.finishedAt < RESCAN_GAP_MS;
    // 上次结果已过保鲜期（或从没扫过）：只投递扫描，页面照常先渲染。
    if (!fresh) await rescan(false);
  })().finally(() => {
    initPromise = null;
  });
  return initPromise;
}

/** 清理缓存；成功后返回实际释放字节数。 */
async function clean(categories: string[]): Promise<number> {
  const result = await storageClean(categories);
  await refreshSnapshot();
  return result.freedBytes;
}

const state = computed(() => snapshot.value?.state ?? "idle");
const scanning = computed(() => state.value === "running" || starting.value);
const ready = computed(() => state.value === "ready");
const failed = computed(() => state.value === "failed");
const categories = computed<StorageCategoryScan[]>(() => snapshot.value?.categories ?? []);
const totalBytes = computed(() => (ready.value ? (snapshot.value?.totalBytes ?? null) : null));
const files = computed(() => snapshot.value?.files ?? 0);
const error = computed(() => requestError.value ?? snapshot.value?.error ?? null);
const startedAt = computed(() => snapshot.value?.startedAt ?? null);
const finishedAt = computed(() => snapshot.value?.finishedAt ?? null);

export function useStorageScan() {
  return {
    snapshot: readonly(snapshot),
    progress: readonly(progress),
    categories,
    totalBytes,
    files,
    state,
    scanning,
    ready,
    failed,
    error,
    startedAt,
    finishedAt,
    init,
    rescan,
    clean,
  };
}
