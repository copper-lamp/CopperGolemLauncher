<script setup lang="ts">
// 下载页：全量下载列表，分「正在下载」与「历史下载」两区。
//
// - 顶部工具栏（面板右上方）：并发数下拉框 + 全部继续/全部暂停按钮。
// - 历史区展示上限取设置 `download.history_limit`（0 = 不显示），
//   只影响展示窗口，不删除任何记录（历史行落库于 `core_download_task`，
//   超出上限由内核按 `HISTORY_KEEP` 淘汰最旧的终态记录）。
// - 历史行在重启后依然可操作：重试 / 继续会把该行就地转回活跃态
//   （内核沿用原 id 复活任务），删除则同时清掉内存任务与持久化记录。
// - 「清除记录」只清终态条目，不碰在跑 / 排队 / 安装中的任务，也不删文件。
//
// 进度显示的三条规则（都来自同一个数据源，前端不做猜测）：
// 1. **已完成的条目不再有进度条与剩余大小**：下载完了还画一条 100% 的条、
//    再标一个「1.2 GB / 1.2 GB」，只是把已经说完的话再说一遍。
// 2. **阶段化进度**（`phaseProgress` 非空）：下载完成之后的安装 / 解包阶段，
//    进度条改由阶段进度驱动，剩余大小的位置换成阶段文案（如「正在解包安装包 ·
//    …/C/d.dll (32/128)」）。游戏安装与 lip 安装都走这条路径。
// 3. **字节进度**：普通下载按已下 / 总量计算，并显示速率。

import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import {
  X,
  Play,
  Pause,
  LoaderCircle,
  AlertTriangle,
  CheckCircle2,
  Ban,
  RotateCw,
  Info,
  PackageCheck,
  FolderOpen,
  Trash2,
} from "@lucide/vue";

import { useDownloads } from "../composables/useDownloads";
import { usePlatform } from "../composables/usePlatform";
import { useSettings } from "../composables/useSettings";
import { useI18n } from "../i18n";
import {
  downloadClearHistory,
  downloadReveal,
  formatBytes,
  formatSpeed,
  type DownloadTask,
} from "../api/download";
import { showToast } from "../composables/useToast";
import {
  contentDownloadRecords,
  contentDownloadRecordsClear,
  type ContentDownloadRecord,
  type ContentInstallProgress,
} from "../modules/content-download/api";
import { gameRetryVersion, gameTaskBindings } from "../modules/game-download/api";
import CoSelect from "../components/ui/CoSelect.vue";
import CoButton from "../components/ui/CoButton.vue";

const { t } = useI18n();
const {
  tasks,
  concurrency,
  activeCount,
  pause,
  resume,
  cancel,
  retry,
  remove,
  pauseAll,
  resumeAll,
  setConcurrency,
  refresh,
} = useDownloads();
const { get } = useSettings();
// 移动端没有可唤起的文件管理器（内核侧明确报不支持），按钮直接不出现：
// 给一个点了必然报错的按钮比不给更糟。
const { isMobile } = usePlatform();

/**
 * 活跃状态：仍需排队、正在传输，或传输已完成但还在安装。
 *
 * `installing` 必须算活跃：它既不是「排队中」也不是「下载中」，漏掉它会让
 * 正在安装的条目掉进历史区，进度条刚跑完就从「正在下载」跳走。
 */
const ACTIVE_STATUSES = new Set(["queued", "downloading", "installing"]);

/** 终态成功：下载与安装都已结束，条目不再需要任何进度反馈。 */
const COMPLETED_STATUSES = new Set(["done"]);

/** 按 id 倒序（最新在前）。 */
const sorted = computed(() => [...tasks.value].sort((a, b) => b.id - a.id));

const historyLimit = computed(() => get<number>("download.history_limit", 50));

/**
 * 历史下载：暂停 / 失败 / 取消 / 已完成的任务，按倒序取前 N 条。
 * 上限为 0 时整区不渲染（用户选择「不显示」）。
 */
const contentRecords = ref<ContentDownloadRecord[]>([]);
let refreshContentRecords: (() => void) | undefined;

/** 记录 id → 当前阶段细节（lipd 的实时子步骤名，只活在本次会话里）。 */
const installDetails = ref<Record<string, string>>({});

/**
 * 下载任务 id → 游戏版本 id。
 *
 * 下载中心列的是核心下载任务，安装流水线却按版本 id 取记录。映射由内核给出
 * （见 `gameTaskBindings`），前端不依据 dest / 文件名猜测——猜错会把安装指向
 * 另一个版本。缺失的条目不显示安装按钮，而不是显示一个点了就报错的按钮。
 */
const gameBindings = ref<Record<number, string>>({});

/** 该条目是否可手动触发安装：整包已落盘、且不处于下载中 / 安装中。 */
function installableVersion(task: DownloadTask): string | null {
  if (task.status === "installing") return null;
  if (task.status !== "done" && task.status !== "failed") return null;
  return gameBindings.value[task.id] ?? null;
}

/**
 * 内容下载记录 → 下载条目视图（阶段进度与阶段文案随之带上）。
 *
 * 阶段细节（lipd 当前子步骤名）只存在于事件里，不落库：它是「此刻在干什么」
 * 的瞬时信息，重启后既没有意义也不该被当成历史展示。
 */
function contentRecordTask(record: ContentDownloadRecord, index: number): DownloadTask {
  return {
    id: Number.MAX_SAFE_INTEGER - index,
    filename: record.name,
    url: `${record.source}:${record.id}`,
    dest: record.dest ?? "",
    total_bytes: 0,
    downloaded_bytes: 0,
    speed_bytes_per_sec: 0,
    status:
      record.state === "installing"
        ? "installing"
        : record.state === "installed"
          ? "done"
          : "failed",
    error: record.error,
    retry_count: 0,
    phase_progress: record.progress ?? (record.state === "installing" ? 0 : null),
    stage: record.stage ?? null,
    stage_detail: installDetails.value[record.id] ?? null,
  };
}

const historyTasks = computed(() => {
  const limit = historyLimit.value;
  if (limit <= 0) return [];
  const contentHistory: DownloadTask[] = contentRecords.value
    .filter((record) => !record.taskId && record.state !== "installing")
    .map(contentRecordTask);
  return [...sorted.value.filter((task) => !ACTIVE_STATUSES.has(task.status)), ...contentHistory].slice(0, limit);
});

const activeTasks = computed(() => [
  ...sorted.value.filter((task) => ACTIVE_STATUSES.has(task.status)),
  ...contentRecords.value
    .filter((record) => record.state === "installing")
    .map(contentRecordTask),
]);

const hasAnyActive = computed(() => activeCount() > 0);

const concurrencyOptions = [1, 2, 3, 4, 5].map((n) => ({
  value: String(n),
  label: t("download.concurrency_unit", { count: n }),
}));

/**
 * 进度条宽度（%）；`null` 表示这条目没有进度条可画。
 *
 * - 阶段进度优先（安装 / 解包等与字节无关的阶段）；
 * - 终态成功一律不画（下载完了没有「进度」可言）；
 * - 其余按字节算，总量未知时返回 `null`（宁可不画，也不画一条永远 0% 的假条）。
 */
function progressOf(task: DownloadTask): number | null {
  if (COMPLETED_STATUSES.has(task.status)) return null;
  if (task.phase_progress != null) return Math.min(100, Math.max(0, task.phase_progress * 100));
  if (task.total_bytes <= 0) return null;
  return Math.min(100, (task.downloaded_bytes / task.total_bytes) * 100);
}

/** 阶段文案：i18n 译文 + 动态细节。无阶段信息时返回 `null`。 */
function stageText(task: DownloadTask): string | null {
  if (!task.stage) return null;
  const label = t(task.stage);
  return task.stage_detail ? `${label} · ${task.stage_detail}` : label;
}

/**
 * 进度区文案；`null` 表示这个位置什么都不该显示。
 *
 * 已完成的条目按产品要求不再显示剩余大小：那句话的答案永远是「0」。
 */
function metaText(task: DownloadTask): string | null {
  if (COMPLETED_STATUSES.has(task.status)) return null;
  // 阶段化进度：剩余大小的位置让给阶段提示（正在解包哪个文件 / 第几步）。
  const stage = stageText(task);
  if (stage) return stage;
  if (task.total_bytes <= 0) {
    return task.status === "downloading"
      ? formatSpeed(task.speed_bytes_per_sec)
      : t("download.unknown_size");
  }
  const total = formatBytes(task.total_bytes);
  const base = `${formatBytes(task.downloaded_bytes)} / ${total}`;
  return task.status === "downloading"
    ? `${base} · ${formatSpeed(task.speed_bytes_per_sec)}`
    : base;
}

function statusLabel(task: DownloadTask): string {
  return t(`download.status.${task.status}`);
}

function statusIcon(task: DownloadTask) {
  switch (task.status) {
    case "downloading":
      return { icon: LoaderCircle, cls: "is-downloading" };
    case "paused":
      return { icon: Pause, cls: "is-paused" };
    case "failed":
      return { icon: AlertTriangle, cls: "is-failed" };
    case "done":
      return { icon: CheckCircle2, cls: "is-done" };
    case "cancelled":
      return { icon: Ban, cls: "is-cancelled" };
    case "installing":
      return { icon: LoaderCircle, cls: "is-downloading" };
    default:
      return { icon: LoaderCircle, cls: "is-queued" };
  }
}

function toggleAll() {
  if (hasAnyActive.value) void pauseAll();
  else void resumeAll();
}

function changeConcurrency(value: string) {
  void setConcurrency(Number(value));
}

async function handleRemove(id: number) {
  try {
    await remove(id);
  } catch (e) {
    showToast(String(e), "error");
  }
}

/**
 * 手动触发安装（只重装，不重下）。
 *
 * 该版本下所有未装好的实例会被重新排进安装。失败原因原样呈现给用户：安装链的失败
 * 原因（商店授权、设备注册、解包、加载器）才是用户下一步要依据的信息，替换成统一
 * 文案等于把排查线索丢掉。
 */
async function handleInstall(task: DownloadTask) {
  const versionId = installableVersion(task);
  if (!versionId) return;
  try {
    await gameRetryVersion(versionId);
  } catch (e) {
    showToast(String(e), "error");
  }
}

/**
 * 在文件管理器里定位下载产物。
 *
 * 落点为空（如某些失败记录还没有目标路径）时直接提示，而不是让命令层去报
 * 一个「路径不存在」——两种情况的用户动作完全不同。
 */
async function handleReveal(task: DownloadTask) {
  if (!task.dest) {
    showToast(t("download.reveal_missing"), "error");
    return;
  }
  try {
    await downloadReveal(task.dest);
  } catch (e) {
    showToast(String(e), "error");
  }
}

/** 查看详情弹窗：只展示快照字段，不做任何写操作。 */
const detailTask = ref<DownloadTask | null>(null);

function openDetail(task: DownloadTask) {
  detailTask.value = task;
}

function closeDetail() {
  detailTask.value = null;
}

// ---------------------------------------------------------------- 清除记录

/** 清除记录确认弹窗（破坏性操作必须二次确认）。 */
const clearOpen = ref(false);
const clearing = ref(false);

function openClear() {
  clearOpen.value = true;
}

function closeClear() {
  if (clearing.value) return;
  clearOpen.value = false;
}

/**
 * 清空下载记录：内核任务与内容下载记录各清各的（模块之间不互相代管数据），
 * 只要有一侧成功就刷新列表。删除的是记录，不是文件。
 */
async function confirmClear() {
  if (clearing.value) return;
  clearing.value = true;
  try {
    await downloadClearHistory();
    await contentDownloadRecordsClear();
    await refresh();
    await loadRecords();
    window.dispatchEvent(new Event("content-download-records-updated"));
    showToast(t("download.history_cleared"), "success");
    clearOpen.value = false;
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    clearing.value = false;
  }
}

// ---------------------------------------------------------------- 数据加载

async function loadRecords() {
  try {
    contentRecords.value = await contentDownloadRecords();
  } catch {
    // 内核未就绪时保留现有列表。
  }
}

/**
 * lip 安装进度事件：就地更新对应记录的进度与阶段。
 *
 * 不整表重拉：安装期间这个事件每 120ms 就来一次，每次都发一次 SQL 全表查询
 * 会白白占住数据库连接。列表挂载时与安装结束后（`content-download-records-updated`）
 * 各有一次全量刷新兜底。
 */
function applyInstallProgress(event: Event) {
  const detail = (event as CustomEvent).detail as Partial<ContentInstallProgress> | undefined;
  if (!detail?.id) return;
  if (typeof detail.stageDetail === "string" && detail.stageDetail) {
    installDetails.value = { ...installDetails.value, [detail.id]: detail.stageDetail };
  }
  const index = contentRecords.value.findIndex((record) => record.id === detail.id);
  if (index < 0) return;
  const current = contentRecords.value[index];
  contentRecords.value[index] = {
    ...current,
    progress: typeof detail.progress === "number" ? detail.progress : current.progress,
    stage: detail.stage ?? current.stage,
  };
}

onMounted(() => {
  const load = () => {
    void loadRecords();
    // 绑定关系与记录同源刷新：新增一次游戏下载后映射才会出现，无需另接事件。
    void gameTaskBindings()
      .then((bindings) => {
        const map: Record<number, string> = {};
        for (const binding of bindings) map[binding.task_id] = binding.version_id;
        gameBindings.value = map;
      })
      .catch(() => null);
  };
  refreshContentRecords = load;
  load();
  window.addEventListener("content-download-records-updated", load);
  window.addEventListener("content-download-install-progress", applyInstallProgress);
});

onBeforeUnmount(() => {
  window.removeEventListener("content-download-records-updated", refreshContentRecords ?? (() => null));
  window.removeEventListener("content-download-install-progress", applyInstallProgress);
  refreshContentRecords = undefined;
});
</script>

<template>
  <div class="downloads">
    <header class="downloads__toolbar">
      <label class="downloads__concurrency" :title="t('download.concurrency_hint')">
        <span class="downloads__concurrency-label">{{ t("download.concurrency") }}</span>
        <CoSelect
          :model-value="String(concurrency)"
          :options="concurrencyOptions"
          @update:model-value="changeConcurrency"
        />
      </label>
      <CoButton :variant="hasAnyActive ? 'secondary' : 'primary'" size="sm" @click="toggleAll">
        <Pause v-if="hasAnyActive" :size="15" />
        <Play v-else :size="15" />
        <span>{{ hasAnyActive ? t("download.all_pause") : t("download.all_resume") }}</span>
      </CoButton>
    </header>

    <section v-if="activeTasks.length > 0" class="downloads__section">
      <h2 class="downloads__section-title">{{ t("download.active_section") }}</h2>
      <ul class="downloads__list">
        <li v-for="task in activeTasks" :key="task.id" class="downloads__item">
          <div class="downloads__item-main">
            <component
              :is="statusIcon(task).icon"
              :size="18"
              class="downloads__item-icon"
              :class="[statusIcon(task).cls, { spin: task.status === 'downloading' || task.status === 'queued' }]"
            />
            <div class="downloads__item-body">
              <div class="downloads__item-row">
                <span class="downloads__item-name" :title="task.url">
                  {{ task.filename ?? task.url }}
                </span>
                <span class="downloads__item-status">{{ statusLabel(task) }}</span>
              </div>
              <div class="downloads__item-row downloads__item-row--meta">
                <div v-if="progressOf(task) !== null" class="downloads__item-progress">
                  <div
                    class="downloads__item-progress-bar"
                    :style="{ width: `${progressOf(task)}%` }"
                  />
                </div>
                <span v-if="metaText(task)" class="downloads__item-meta">{{ metaText(task) }}</span>
              </div>
            </div>
          </div>
          <div class="downloads__item-actions">
            <button
              v-if="!isMobile"
              class="downloads__action"
              :title="t('download.actions.reveal')"
              @click="handleReveal(task)"
            >
              <FolderOpen :size="15" />
            </button>
            <button
              v-if="task.status !== 'installing'"
              class="downloads__action"
              :title="t('download.actions.pause')"
              @click="pause(task.id)"
            >
              <Pause :size="15" />
            </button>
            <button
              v-if="task.status !== 'installing'"
              class="downloads__action"
              :title="t('download.actions.cancel')"
              @click="cancel(task.id)"
            >
              <X :size="15" />
            </button>
          </div>
        </li>
      </ul>
    </section>

    <section v-if="activeTasks.length === 0" class="downloads__empty">
      {{ t("download.empty") }}
    </section>

    <section v-if="historyLimit > 0" class="downloads__section">
      <div class="downloads__section-header">
        <h2 class="downloads__section-title">{{ t("download.history_section") }}</h2>
        <button
          v-if="historyTasks.length > 0"
          class="downloads__action downloads__action--labeled"
          :title="t('download.history_clear_hint')"
          @click="openClear"
        >
          <Trash2 :size="15" />
          <span class="downloads__action-label">{{ t("download.history_clear") }}</span>
        </button>
      </div>
      <div v-if="historyTasks.length === 0" class="downloads__empty downloads__empty--inline">
        {{ t("download.history_empty") }}
      </div>
      <ul v-else class="downloads__list">
        <li v-for="task in historyTasks" :key="task.id" class="downloads__item">
          <div class="downloads__item-main">
            <component
              :is="statusIcon(task).icon"
              :size="18"
              class="downloads__item-icon"
              :class="statusIcon(task).cls"
            />
            <div class="downloads__item-body">
              <div class="downloads__item-row">
                <span class="downloads__item-name" :title="task.url">
                  {{ task.filename ?? task.url }}
                </span>
                <span class="downloads__item-status">{{ statusLabel(task) }}</span>
              </div>
              <div
                v-if="progressOf(task) !== null || metaText(task)"
                class="downloads__item-row downloads__item-row--meta"
              >
                <div v-if="progressOf(task) !== null" class="downloads__item-progress">
                  <div
                    class="downloads__item-progress-bar"
                    :style="{ width: `${progressOf(task)}%` }"
                  />
                </div>
                <span v-if="metaText(task)" class="downloads__item-meta">{{ metaText(task) }}</span>
              </div>
            </div>
          </div>
          <div class="downloads__item-actions">
            <button
              v-if="installableVersion(task)"
              class="downloads__action downloads__action--accent"
              :title="t('download.actions_install_hint')"
              @click="handleInstall(task)"
            >
              <PackageCheck :size="15" />
            </button>
            <button
              v-if="task.status === 'paused'"
              class="downloads__action"
              :title="t('download.actions.resume')"
              @click="resume(task.id)"
            >
              <Play :size="15" />
            </button>
            <button
              v-else-if="task.status === 'failed' || task.status === 'cancelled'"
              class="downloads__action"
              :title="t('download.actions.retry')"
              @click="retry(task.id)"
            >
              <RotateCw :size="15" />
            </button>
            <button
              v-if="!isMobile"
              class="downloads__action"
              :title="t('download.actions.reveal')"
              @click="handleReveal(task)"
            >
              <FolderOpen :size="15" />
            </button>
            <button
              class="downloads__action"
              :title="t('download.actions_view')"
              @click="openDetail(task)"
            >
              <Info :size="15" />
            </button>
            <button
              class="downloads__action"
              :title="t('download.actions.remove')"
              @click="handleRemove(task.id)"
            >
              <X :size="15" />
            </button>
          </div>
        </li>
      </ul>
    </section>

    <Teleport to="body">
      <div
        v-if="detailTask"
        class="downloads__dialog"
        role="dialog"
        aria-modal="true"
        @click.self="closeDetail"
      >
        <div class="downloads__dialog-card">
          <header class="downloads__dialog-header">
            <h2 class="downloads__dialog-title">{{ t("download.detail_title") }}</h2>
            <button class="downloads__action" :title="t('common.close')" @click="closeDetail">
              <X :size="16" />
            </button>
          </header>
          <dl class="downloads__dialog-body">
            <div class="downloads__dialog-row">
              <dt>{{ t("download.detail_name") }}</dt>
              <dd>{{ detailTask.filename ?? t("common.unknown") }}</dd>
            </div>
            <div class="downloads__dialog-row">
              <dt>{{ t("download.detail_url") }}</dt>
              <dd class="downloads__dialog-mono">{{ detailTask.url }}</dd>
            </div>
            <div class="downloads__dialog-row">
              <dt>{{ t("download.detail_dest") }}</dt>
              <dd class="downloads__dialog-mono">{{ detailTask.dest }}</dd>
            </div>
            <div class="downloads__dialog-row">
              <dt>{{ t("download.detail_size") }}</dt>
              <dd>
                {{
                  detailTask.total_bytes > 0
                    ? `${formatBytes(detailTask.downloaded_bytes)} / ${formatBytes(detailTask.total_bytes)}`
                    : t("download.unknown_size")
                }}
              </dd>
            </div>
            <div class="downloads__dialog-row">
              <dt>{{ t("download.detail_progress") }}</dt>
              <dd>
                {{
                  progressOf(detailTask) === null
                    ? t("download.unknown_size")
                    : `${progressOf(detailTask)!.toFixed(1)}%`
                }}
              </dd>
            </div>
            <div class="downloads__dialog-row">
              <dt>{{ t("download.detail_retry_count") }}</dt>
              <dd>{{ detailTask.retry_count }}</dd>
            </div>
            <div v-if="detailTask.error" class="downloads__dialog-row">
              <dt>{{ t("download.detail_error") }}</dt>
              <dd class="downloads__dialog-error">{{ detailTask.error }}</dd>
            </div>
          </dl>
          <footer class="downloads__dialog-footer">
            <CoButton variant="ghost" @click="closeDetail">{{ t("common.close") }}</CoButton>
          </footer>
        </div>
      </div>

      <!-- 清除记录：破坏性操作，二次确认后才动手 -->
      <div
        v-if="clearOpen"
        class="downloads__dialog"
        role="dialog"
        aria-modal="true"
        @click.self="closeClear"
      >
        <div class="downloads__dialog-card downloads__dialog-card--warning">
          <header class="downloads__dialog-header">
            <h2 class="downloads__dialog-title">{{ t("download.history_clear_title") }}</h2>
            <button class="downloads__action" :title="t('common.close')" @click="closeClear">
              <X :size="16" />
            </button>
          </header>
          <div class="downloads__warning">
            <AlertTriangle :size="18" class="downloads__warning-icon" />
            <p class="downloads__warning-text">{{ t("download.history_clear_body") }}</p>
          </div>
          <footer class="downloads__dialog-footer">
            <CoButton variant="ghost" :disabled="clearing" @click="closeClear">
              {{ t("common.cancel") }}
            </CoButton>
            <CoButton variant="danger" :disabled="clearing" @click="confirmClear">
              {{ t("download.history_clear_confirm") }}
            </CoButton>
          </footer>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.downloads {
  height: 100%;
  padding: var(--copper-space-5);
  overflow-y: auto;
}

.downloads__toolbar {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--copper-space-3);
  margin-bottom: var(--copper-space-4);
}

.downloads__concurrency {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.downloads__concurrency-label {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.downloads__section {
  margin-bottom: var(--copper-space-5);
}

/* 区标题与右侧操作（清除记录）同排 */
.downloads__section-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  margin-bottom: var(--copper-space-2);
}

.downloads__section-header .downloads__section-title {
  margin-bottom: 0;
}

.downloads__section-title {
  margin-bottom: var(--copper-space-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
  letter-spacing: 0.4px;
  text-transform: uppercase;
}

.downloads__empty {
  padding: var(--copper-space-6);
  text-align: center;
  color: var(--copper-text-secondary);
}

.downloads__empty--inline {
  padding: var(--copper-space-4);
  font-size: var(--copper-font-size-sm);
}

.downloads__list {
  list-style: none;
  display: flex;
  flex-direction: column;
}

/* 条目默认透明无边框；悬浮时才升为带阴影的卡片，避免长列表视觉噪音 */
.downloads__item {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) var(--copper-space-4);
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--copper-radius-lg);
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.downloads__item:hover {
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow);
}

/* 操作区默认隐藏，悬浮或键盘聚焦时才出现 */
.downloads__item-actions {
  display: flex;
  gap: var(--copper-space-1);
  flex-shrink: 0;
  opacity: 0;
  transition: opacity var(--copper-duration-fast) var(--copper-easing);
}

.downloads__item:hover .downloads__item-actions,
.downloads__item:focus-within .downloads__item-actions {
  opacity: 1;
}

.downloads__item-main {
  flex: 1;
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  min-width: 0;
}

.downloads__item-icon {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
}

.downloads__item-icon.is-downloading {
  color: var(--copper-accent);
}

.downloads__item-icon.is-queued {
  color: var(--copper-text-secondary);
}

.downloads__item-icon.is-paused {
  color: var(--copper-warning);
}

.downloads__item-icon.is-failed {
  color: var(--copper-danger);
}

.downloads__item-icon.is-done {
  color: var(--copper-success);
}

.spin {
  animation: spin 1.2s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.downloads__item-body {
  flex: 1;
  min-width: 0;
}

.downloads__item-row {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.downloads__item-row--meta {
  margin-top: var(--copper-space-1);
  gap: var(--copper-space-3);
}

.downloads__item-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--copper-font-size-md);
}

.downloads__item-status {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.downloads__item-progress {
  flex: 1;
  height: 5px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-3);
  overflow: hidden;
}

.downloads__item-progress-bar {
  height: 100%;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  transition: width var(--copper-duration) var(--copper-easing);
}

.downloads__item-meta {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.downloads__action {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.downloads__action:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

/* 安装入口是主动作，与暂停 / 重试等次级操作区分开 */
.downloads__action--accent {
  color: var(--copper-accent);
}

.downloads__action--accent:hover {
  background: color-mix(in srgb, var(--copper-accent) 16%, transparent);
  color: var(--copper-accent);
}

/* 带文字的操作按钮（清除记录）：icon 按钮的加宽形态 */
.downloads__action--labeled {
  width: auto;
  padding: 0 var(--copper-space-2);
  gap: var(--copper-space-1);
}

.downloads__action-label {
  font-size: var(--copper-font-size-xs);
}

.downloads__dialog {
  position: fixed;
  inset: 0;
  z-index: 100;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--copper-overlay);
  animation: downloads-fade var(--copper-duration) var(--copper-easing);
}

.downloads__dialog-card {
  width: min(480px, calc(100vw - 48px));
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  box-shadow: var(--copper-shadow);
  animation: downloads-pop var(--copper-duration) var(--copper-easing);
  overflow: hidden;
}

.downloads__dialog-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--copper-space-4) var(--copper-space-4) 0;
}

.downloads__dialog-title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
}

.downloads__dialog-body {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  padding: var(--copper-space-4);
}

.downloads__dialog-row {
  display: grid;
  grid-template-columns: 96px 1fr;
  gap: var(--copper-space-3);
  align-items: start;
}

.downloads__dialog-row dt {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.downloads__dialog-row dd {
  margin: 0;
  font-size: var(--copper-font-size-md);
  word-break: break-all;
}

.downloads__dialog-mono {
  font-family: "Cascadia Mono", "Consolas", monospace;
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.downloads__dialog-error {
  color: var(--copper-danger);
}

.downloads__dialog-footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4);
  background: var(--copper-surface-2);
}

/* 清除记录弹窗：警示腰带把「这会删东西」说在按钮之前 */
.downloads__dialog-card--warning {
  width: min(420px, calc(100vw - 48px));
}

.downloads__warning {
  display: flex;
  align-items: flex-start;
  gap: var(--copper-space-3);
  padding: var(--copper-space-4);
}

.downloads__warning-icon {
  flex-shrink: 0;
  color: var(--copper-warning);
}

.downloads__warning-text {
  margin: 0;
  font-size: var(--copper-font-size-md);
  line-height: 1.6;
  color: var(--copper-text);
}

@keyframes downloads-fade {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

@keyframes downloads-pop {
  from {
    opacity: 0;
    transform: scale(0.96);
  }
  to {
    opacity: 1;
    transform: scale(1);
  }
}
</style>
