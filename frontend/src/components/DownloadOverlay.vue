<script setup lang="ts">
// 下载悬浮窗：左导航底部入口，约半窗悬浮，展示活跃下载项与全局控制。

import { computed, onMounted, ref } from "vue";
import {
  ArrowDownToLine,
  X,
  Pause,
  Play,
  LoaderCircle,
  AlertTriangle,
  CheckCircle2,
  Ban,
  RotateCw,
} from "@lucide/vue";

import { useDownloads } from "../composables/useDownloads";
import { useI18n } from "../i18n";
import { formatBytes, formatSpeed, type DownloadTask } from "../api/download";

const { t } = useI18n();
const {
  tasks,
  activeCount,
  pause,
  resume,
  cancel,
  retry,
  pauseAll,
  resumeAll,
} = useDownloads();

const open = ref(false);

onMounted(async () => {
  // 依赖 useDownloads 的全局单例，此处仅确保初始化已发生（App 启动时已调）。
});

const hasAnyActive = computed(() => activeCount() > 0);
const sorted = computed(() =>
  [...tasks.value].sort((a, b) => b.id - a.id),
);

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
      return { icon: ArrowDownToLine, cls: "is-queued" };
  }
}

function toggleAll() {
  if (activeCount() > 0) void pauseAll();
  else void resumeAll();
}
</script>

<template>
  <div class="download-overlay">
    <button
      class="side-nav__item download-overlay__trigger"
      :title="t('nav.downloads')"
      @click="open = !open"
    >
      <ArrowDownToLine :size="20" />
      <span v-if="activeCount() > 0" class="download-overlay__badge">
        {{ activeCount() }}
      </span>
    </button>

    <Transition name="pop">
      <div v-if="open" class="download-overlay__panel">
        <header class="download-overlay__header">
          <span class="download-overlay__title">{{ t("download.title") }}</span>
          <div class="download-overlay__header-actions">
            <button
              class="download-overlay__icon-btn"
              :title="hasAnyActive ? t('download.all_pause') : t('download.all_resume')"
              @click="toggleAll"
            >
              <Pause v-if="hasAnyActive" :size="15" />
              <Play v-else :size="15" />
            </button>
            <button
              class="download-overlay__icon-btn"
              :title="t('common.close')"
              @click="open = false"
            >
              <X :size="15" />
            </button>
          </div>
        </header>

        <div v-if="sorted.length === 0" class="download-overlay__empty">
          {{ t("download.empty") }}
        </div>

        <ul v-else class="download-overlay__list">
          <li v-for="task in sorted" :key="task.id" class="download-item">
            <div class="download-item__row">
              <component
                :is="statusIcon(task).icon"
                :size="15"
                class="download-item__status"
                :class="[statusIcon(task).cls, { spin: task.status === 'downloading' }]"
              />
              <span class="download-item__name" :title="task.url">
                {{ task.filename ?? task.url }}
              </span>
              <span class="download-item__status-text">{{ statusLabel(task) }}</span>
            </div>
            <div class="download-item__row download-item__row--meta">
              <div class="download-item__progress">
                <div
                  class="download-item__progress-bar"
                  :style="{ width: `${progressOf(task)}%` }"
                />
              </div>
              <span class="download-item__meta">
                {{
                  task.status === "downloading"
                    ? `${formatBytes(task.downloaded_bytes)} / ${task.total_bytes > 0 ? formatBytes(task.total_bytes) : t("download.unknown_size")} · ${formatSpeed(task.speed_bytes_per_sec)}`
                    : task.total_bytes > 0
                      ? `${formatBytes(task.downloaded_bytes)} / ${formatBytes(task.total_bytes)}`
                      : t("download.unknown_size")
                }}
              </span>
            </div>
            <div class="download-item__row download-item__row--actions">
              <template v-if="task.status === 'downloading' || task.status === 'queued'">
                <button class="download-item__action" @click="pause(task.id)">
                  <Pause :size="13" />
                </button>
              </template>
              <template v-else-if="task.status === 'paused'">
                <button class="download-item__action" @click="resume(task.id)">
                  <Play :size="13" />
                </button>
              </template>
              <template v-else-if="task.status === 'failed'">
                <button class="download-item__action" @click="retry(task.id)">
                  <RotateCw :size="13" />
                </button>
              </template>
              <button class="download-item__action" @click="cancel(task.id)">
                <X :size="13" />
              </button>
            </div>
          </li>
        </ul>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.download-overlay {
  position: relative;
}

.download-overlay__trigger {
  position: relative;
}

.download-overlay__badge {
  position: absolute;
  top: 2px;
  right: 2px;
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
  font-size: 10px;
  font-weight: 600;
  line-height: 16px;
  text-align: center;
}

.download-overlay__panel {
  position: absolute;
  bottom: 52px;
  left: 50%;
  width: min(400px, 60vw);
  max-height: 55vh;
  display: flex;
  flex-direction: column;
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  box-shadow: var(--copper-shadow);
  overflow: hidden;
  z-index: 100;
}

.download-overlay__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--copper-space-3) var(--copper-space-4);
  border-bottom: 1px solid var(--copper-border);
}

.download-overlay__title {
  font-weight: 600;
  font-size: var(--copper-font-size-md);
}

.download-overlay__header-actions {
  display: flex;
  gap: var(--copper-space-1);
}

.download-overlay__icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.download-overlay__icon-btn:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.download-overlay__empty {
  padding: var(--copper-space-6);
  text-align: center;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.download-overlay__list {
  list-style: none;
  padding: var(--copper-space-2);
  overflow-y: auto;
}

.download-item {
  padding: var(--copper-space-2) var(--copper-space-2) var(--copper-space-1);
}

.download-item + .download-item {
  border-top: 1px solid var(--copper-border);
}

.download-item__row {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.download-item__row--meta {
  margin-top: var(--copper-space-1);
  gap: var(--copper-space-3);
}

.download-item__row--actions {
  justify-content: flex-end;
  margin-top: var(--copper-space-1);
}

.download-item__status {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
}

.download-item__status.is-downloading {
  color: var(--copper-accent);
}

.download-item__status.is-paused {
  color: var(--copper-warning);
}

.download-item__status.is-failed {
  color: var(--copper-danger);
}

.download-item__status.is-done {
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

.download-item__name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--copper-font-size-sm);
}

.download-item__status-text {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.download-item__progress {
  flex: 1;
  height: 4px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-3);
  overflow: hidden;
}

.download-item__progress-bar {
  height: 100%;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  transition: width var(--copper-duration) var(--copper-easing);
}

.download-item__meta {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.download-item__action {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.download-item__action:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}
</style>
