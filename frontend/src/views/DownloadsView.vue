<script setup lang="ts">
// 下载页：全量下载列表（下载悬浮窗的精简复刻，用于内容较多的场景）。

import { computed } from "vue";
import {
  Pause,
  Play,
  LoaderCircle,
  AlertTriangle,
  CheckCircle2,
  Ban,
  RotateCw,
  X,
} from "@lucide/vue";

import { useDownloads } from "../composables/useDownloads";
import { useI18n } from "../i18n";
import { formatBytes, formatSpeed, type DownloadTask } from "../api/download";
import { showToast } from "../composables/useToast";

const { t } = useI18n();
const { tasks, activeCount, pause, resume, cancel, retry, remove, pauseAll, resumeAll } =
  useDownloads();

const sorted = computed(() => [...tasks.value].sort((a, b) => b.id - a.id));
const hasAnyActive = computed(() => activeCount() > 0);

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

function toggleAll() {
  if (activeCount() > 0) void pauseAll();
  else void resumeAll();
}

async function handleRemove(id: number) {
  try {
    await remove(id);
  } catch (e) {
    showToast(String(e), "error");
  }
}
</script>

<template>
  <div class="downloads">
    <header class="downloads__header">
      <h1 class="downloads__title">{{ t("download.title") }}</h1>
      <button class="downloads__toggle" @click="toggleAll">
        <Pause v-if="hasAnyActive" :size="15" />
        <Play v-else :size="15" />
        <span>{{ hasAnyActive ? t("download.all_pause") : t("download.all_resume") }}</span>
      </button>
    </header>

    <div v-if="sorted.length === 0" class="downloads__empty">
      {{ t("download.empty") }}
    </div>

    <ul v-else class="downloads__list">
      <li v-for="task in sorted" :key="task.id" class="downloads__item">
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
              <span class="downloads__item-meta">
                {{
                  task.status === "downloading"
                    ? `${formatBytes(task.downloaded_bytes)} / ${task.total_bytes > 0 ? formatBytes(task.total_bytes) : t("download.unknown_size")} · ${formatSpeed(task.speed_bytes_per_sec)}`
                    : task.total_bytes > 0
                      ? `${formatBytes(task.downloaded_bytes)} / ${formatBytes(task.total_bytes)}`
                      : t("download.unknown_size")
                }}
              </span>
            </div>
          </div>
        </div>
        <div class="downloads__item-actions">
          <template v-if="task.status === 'downloading' || task.status === 'queued'">
            <button class="downloads__action" :title="t('download.actions.pause')" @click="pause(task.id)">
              <Pause :size="15" />
            </button>
          </template>
          <template v-else-if="task.status === 'paused'">
            <button class="downloads__action" :title="t('download.actions.resume')" @click="resume(task.id)">
              <Play :size="15" />
            </button>
          </template>
          <template v-else-if="task.status === 'failed'">
            <button class="downloads__action" :title="t('download.actions.retry')" @click="retry(task.id)">
              <RotateCw :size="15" />
            </button>
          </template>
          <template v-if="task.status === 'failed' || task.status === 'done' || task.status === 'cancelled'">
            <button class="downloads__action" :title="t('download.actions.remove')" @click="handleRemove(task.id)">
              <X :size="15" />
            </button>
          </template>
          <template v-else>
            <button class="downloads__action" :title="t('download.actions.cancel')" @click="cancel(task.id)">
              <X :size="15" />
            </button>
          </template>
        </div>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.downloads {
  height: 100%;
  padding: var(--copper-space-5);
  overflow-y: auto;
}

.downloads__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--copper-space-4);
}

.downloads__title {
  font-size: var(--copper-font-size-xl);
  font-weight: 700;
}

.downloads__toggle {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.downloads__toggle:hover {
  background: var(--copper-surface-2);
}

.downloads__empty {
  padding: var(--copper-space-6);
  text-align: center;
  color: var(--copper-text-secondary);
}

.downloads__list {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.downloads__item {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) var(--copper-space-4);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
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

.downloads__item-actions {
  display: flex;
  gap: var(--copper-space-1);
  flex-shrink: 0;
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
</style>
