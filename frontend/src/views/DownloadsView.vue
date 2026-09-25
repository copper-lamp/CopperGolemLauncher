<script setup lang="ts">
// 下载页：全量下载列表，分「正在下载」与「历史下载」两区。
//
// - 顶部工具栏（面板右上方）：并发数下拉框 + 全部继续/全部暂停按钮。
//   此前按钮经 Teleport 注入全局标题栏，现按要求回落为面板内控件，
//   与并发数一并构成下载页的操作区。
// - 历史区展示上限取设置 `download.history_limit`（0 = 不显示），
//   只影响展示窗口，不删除任何记录（历史记录本身仅存在于内核内存）。

import { computed, ref } from "vue";
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
} from "@lucide/vue";

import { useDownloads } from "../composables/useDownloads";
import { useSettings } from "../composables/useSettings";
import { useI18n } from "../i18n";
import { formatBytes, formatSpeed, type DownloadTask } from "../api/download";
import { showToast } from "../composables/useToast";
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
} = useDownloads();
const { get } = useSettings();

/** 活跃状态：仍需排队或正在传输。 */
const ACTIVE_STATUSES = new Set(["queued", "downloading"]);

/** 按 id 倒序（最新在前）。 */
const sorted = computed(() => [...tasks.value].sort((a, b) => b.id - a.id));

const activeTasks = computed(() => sorted.value.filter((task) => ACTIVE_STATUSES.has(task.status)));

const historyLimit = computed(() => get<number>("download.history_limit", 50));

/**
 * 历史下载：暂停 / 失败 / 取消 / 已完成的任务，按倒序取前 N 条。
 * 上限为 0 时整区不渲染（用户选择「不显示」）。
 */
const historyTasks = computed(() => {
  const limit = historyLimit.value;
  if (limit <= 0) return [];
  return sorted.value.filter((task) => !ACTIVE_STATUSES.has(task.status)).slice(0, limit);
});

const hasAnyActive = computed(() => activeCount() > 0);

const concurrencyOptions = [1, 2, 3, 4, 5].map((n) => ({
  value: String(n),
  label: t("download.concurrency_unit", { count: n }),
}));

function progressOf(task: DownloadTask): number {
  if (task.total_bytes <= 0) return 0;
  return Math.min(100, (task.downloaded_bytes / task.total_bytes) * 100);
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
    default:
      return { icon: LoaderCircle, cls: "is-queued" };
  }
}

/** 进度区文案：下载中带速率，其余只展示已下 / 总量。 */
function metaText(task: DownloadTask): string {
  const total =
    task.total_bytes > 0 ? formatBytes(task.total_bytes) : t("download.unknown_size");
  const base = `${formatBytes(task.downloaded_bytes)} / ${total}`;
  return task.status === "downloading"
    ? `${base} · ${formatSpeed(task.speed_bytes_per_sec)}`
    : base;
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

/** 查看详情弹窗：只展示快照字段，不做任何写操作。 */
const detailTask = ref<DownloadTask | null>(null);

function openDetail(task: DownloadTask) {
  detailTask.value = task;
}

function closeDetail() {
  detailTask.value = null;
}
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
                <div class="downloads__item-progress">
                  <div
                    class="downloads__item-progress-bar"
                    :style="{ width: `${progressOf(task)}%` }"
                  />
                </div>
                <span class="downloads__item-meta">{{ metaText(task) }}</span>
              </div>
            </div>
          </div>
          <div class="downloads__item-actions">
            <button
              class="downloads__action"
              :title="t('download.actions.pause')"
              @click="pause(task.id)"
            >
              <Pause :size="15" />
            </button>
            <button
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
      <h2 class="downloads__section-title">{{ t("download.history_section") }}</h2>
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
              <div class="downloads__item-row downloads__item-row--meta">
                <div class="downloads__item-progress">
                  <div
                    class="downloads__item-progress-bar"
                    :style="{ width: `${progressOf(task)}%` }"
                  />
                </div>
                <span class="downloads__item-meta">{{ metaText(task) }}</span>
              </div>
            </div>
          </div>
          <div class="downloads__item-actions">
            <button
              v-if="task.status === 'paused'"
              class="downloads__action"
              :title="t('download.actions.resume')"
              @click="resume(task.id)"
            >
              <Play :size="15" />
            </button>
            <button
              v-else-if="task.status === 'failed'"
              class="downloads__action"
              :title="t('download.actions.retry')"
              @click="retry(task.id)"
            >
              <RotateCw :size="15" />
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
              <dd>{{ `${progressOf(detailTask).toFixed(1)}%` }}</dd>
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
  padding: var(--copper-space-3) var(--copper-space-4);
  background: var(--copper-surface-2);
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
