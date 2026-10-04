<script setup lang="ts">
// 更新弹窗：问「是否现在更新并重启」。
//
// 开合由 `panelOpen` 单例控制（标题栏的更新按钮打开），
// **不参与启动流程**：启动期的后台检查只写状态，用户不点就没有任何界面。
//
// 弹窗要回答的问题只有一个：「现在更新并重启，还是稍后？」因此它必须同时给出
// 决策所需的全部事实 —— 新版本号、包大小、已下载/已就绪、发行说明、失败原因。
// 相位为 `downloading` 时按钮区域退化为「查看下载进度 / 取消」，不给重启选项。
import { computed, onUnmounted, ref, watch } from "vue";
import {
  AlertTriangle,
  ArrowRight,
  CheckCircle2,
  Download,
  ExternalLink,
  LoaderCircle,
  RefreshCw,
} from "@lucide/vue";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";

import CoButton from "../ui/CoButton.vue";
import CoProgress from "../ui/CoProgress.vue";
import { useUpdate } from "../../composables/useUpdate";
import { showToast } from "../../composables/useToast";
import { formatBytes, formatSpeed } from "../../api/download";
import { formatEta, formatTime } from "./updateFormat";
import { useI18n } from "../../i18n";

const { t } = useI18n();
const {
  phase,
  status,
  latest,
  progress,
  busy,
  error,
  errorKind,
  autoInstallable,
  versionDelta,
  hasNotes,
  panelOpen,
  closePanel,
  check,
  cancel,
  install,
} = useUpdate();

/** 动作异常（IPC 层；后端业务失败走 `error`）。 */
const actionError = ref<string | null>(null);

const downloading = computed(() => phase.value === "downloading");
const downloaded = computed(() => phase.value === "downloaded");
const failed = computed(() => phase.value === "failed");
const checking = computed(() => phase.value === "checking");
const available = computed(() => phase.value === "available");

/** 需要人工下载安装（`manual` 形态）。 */
const manualOnly = computed(() => latest.value != null && !autoInstallable.value);

/** 进度比例：总量未知时 null → 不确定态。 */
const progressRatio = computed(() => progress.value?.ratio ?? null);

const title = computed(() => {
  if (downloaded.value) return t("update.dialog.ready_title");
  if (downloading.value) return t("update.dialog.downloading_title");
  if (failed.value) return t("update.dialog.failed_title");
  if (checking.value) return t("update.dialog.checking_title");
  if (available.value) return t("update.dialog.available_title");
  return t("update.dialog.idle_title");
});

const icon = computed(() => {
  if (downloaded.value) return CheckCircle2;
  if (failed.value) return AlertTriangle;
  return Download;
});

function describe(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function failureHint(): string {
  if (errorKind.value === "no_asset") return t("update.failed_no_asset_hint");
  if (errorKind.value === "download") return t("update.failed_download_hint");
  return t("update.failed_network_hint");
}

/** 每次打开清掉上一次的残留状态，否则关掉再开会看到旧的报错。 */
watch(panelOpen, (open) => {
  if (open) actionError.value = null;
});

/**
 * Esc 关闭。
 *
 * 弹窗是遮罩层，点空白处能关，但用户按 Esc 才是「我要放弃这个弹窗」的第一反应。
 * 监听挂在 window 上：焦点在按钮时按 Esc 不触发冒泡到遮罩的 keydown（默认不可靠），
 * 直接监听最稳，且必须在关闭时解绑，否则每次开合都会多挂一个监听。
 */
function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") closePanel();
}

watch(panelOpen, (open) => {
  if (open) window.addEventListener("keydown", onKeydown);
  else window.removeEventListener("keydown", onKeydown);
});

onUnmounted(() => window.removeEventListener("keydown", onKeydown));

async function onCheck() {
  actionError.value = null;
  try {
    await check();
  } catch (e) {
    actionError.value = describe(e);
  }
}

async function onCancel() {
  actionError.value = null;
  try {
    await cancel();
  } catch (e) {
    actionError.value = describe(e);
  }
}

/**
 * 确认重启更新。
 *
 * 两次点击才生效：第一次把按钮切换成「再次点击确认」，第二次才真的执行。
 * 更新会让当前进程退出、重装启动器 —— 误触的代价太高。
 */
const confirming = ref(false);

async function onInstall() {
  if (!confirming.value) {
    confirming.value = true;
    return;
  }
  actionError.value = null;
  await install();
  confirming.value = false;
}

/** 下载中若点了「现在更新」，回到未确认态：进度会变，按钮文案不该停在确认态。 */
watch(downloaded, (isDone) => {
  if (!isDone) confirming.value = false;
});

async function openReleasePage() {
  const url = latest.value?.html_url;
  if (!url) return;
  await openExternal(url).catch(() => showToast(t("update.open_page_failed"), "error"));
}
</script>

<template>
  <Teleport to="body">
    <div v-if="panelOpen" class="update-dialog" @click.self="closePanel">
      <div class="update-dialog__card" role="dialog" aria-modal="true">
        <header class="update-dialog__header">
          <span class="update-dialog__icon" :class="`update-dialog__icon--${phase}`">
            <component :is="icon" :size="16" />
          </span>
          <div class="update-dialog__heading">
            <h2 class="update-dialog__title">{{ title }}</h2>
            <!-- 版本对照：只在真有新旧之分时出现，避免「0.1.0 → 0.1.0」的噪音 -->
            <p v-if="versionDelta" class="update-dialog__delta">
              <span class="update-dialog__version update-dialog__version--old">
                v{{ versionDelta.current }}
              </span>
              <ArrowRight :size="12" />
              <span class="update-dialog__version update-dialog__version--new">
                v{{ versionDelta.next }}
              </span>
              <span v-if="latest?.asset_size" class="update-dialog__size">
                {{ formatBytes(latest.asset_size) }}
              </span>
            </p>
          </div>
          <button class="update-dialog__close" :title="t('common.close')" @click="closePanel">
            <span aria-hidden="true">×</span>
          </button>
        </header>

        <div class="update-dialog__body">
          <!-- 检查中：不确定态进度，明说在等网络 -->
          <template v-if="checking">
            <p class="update-dialog__hint">{{ t("update.checking_hint") }}</p>
            <CoProgress :value="null" />
          </template>

          <!-- 下载中：进度条 + 字节 + 速度 + ETA -->
          <template v-else-if="downloading">
            <div class="update-dialog__stats">
              <span class="update-dialog__figure">
                <template v-if="progress?.total">
                  {{ formatBytes(progress.downloaded) }} / {{ formatBytes(progress.total) }}
                </template>
                <template v-else>{{ formatBytes(progress?.downloaded ?? 0) }}</template>
              </span>
              <span v-if="progress?.speed" class="update-dialog__dim">
                {{ formatSpeed(progress.speed) }}
                <template v-if="progress.etaSeconds != null">
                  · {{ t("update.eta", { seconds: formatEta(progress.etaSeconds) }) }}
                </template>
              </span>
            </div>
            <CoProgress :value="progressRatio" />
            <p class="update-dialog__hint">{{ t("update.downloading_hint") }}</p>
          </template>

          <!-- 已就绪：成功态进度 + 校验通过 -->
          <template v-else-if="downloaded">
            <CoProgress :value="1" tone="success" />
            <p class="update-dialog__hint update-dialog__hint--ready">
              {{ t("update.ready_body") }}
              <span v-if="latest?.sha256" class="update-dialog__checksum">
                {{ t("update.checksum_ok") }}
              </span>
            </p>
          </template>

          <!-- 失败：原文 + 分类提示 -->
          <template v-else-if="failed">
            <p class="update-dialog__error">{{ error ?? t("update.failed_generic") }}</p>
            <p class="update-dialog__hint">{{ failureHint() }}</p>
          </template>

          <!-- 已是最新 -->
          <template v-else-if="status?.phase === 'idle'">
            <p class="update-dialog__hint">
              {{ t("update.up_to_date_body", { version: status.current_version }) }}
            </p>
            <p v-if="status.last_checked_at" class="update-dialog__hint update-dialog__hint--dim">
              {{ t("update.checked_at", { time: formatTime(status.last_checked_at) }) }}
            </p>
          </template>

          <!-- 有新版但自动下载尚未生效（手动触发 / 自动投递失败） -->
          <template v-else-if="available && latest">
            <p class="update-dialog__hint">
              {{ t("update.auto_download_failed_hint") }}
            </p>
          </template>

          <!-- 发行说明 -->
          <div v-if="hasNotes" class="update-dialog__notes">
            <h3 class="update-dialog__notes-title">{{ t("update.notes_title") }}</h3>
            <pre class="update-dialog__notes-body">{{ latest?.notes }}</pre>
          </div>
        </div>

        <p v-if="actionError" class="update-dialog__error update-dialog__error--inline">
          {{ actionError }}
        </p>

        <footer class="update-dialog__footer">
          <button
            v-if="latest?.html_url"
            class="update-dialog__link"
            type="button"
            @click="openReleasePage"
          >
            <ExternalLink :size="13" />
            <span>{{ t("update.view_release") }}</span>
          </button>

          <div class="update-dialog__actions">
            <!-- 已就绪：这是唯一向用户要决策的场合 -->
            <template v-if="downloaded">
              <CoButton variant="ghost" :disabled="busy" @click="closePanel">
                {{ t("update.later") }}
              </CoButton>
              <CoButton
                variant="primary"
                :disabled="busy"
                :class="{ 'update-dialog__confirming': confirming }"
                @click="onInstall"
              >
                <LoaderCircle v-if="busy" class="update-dialog__spin" :size="14" />
                <RefreshCw v-else :size="14" />
                <span>{{ confirming ? t("update.confirm_restart") : t("update.restart_now") }}</span>
              </CoButton>
            </template>

            <!-- 下载中：不给重启选项，只给取消 -->
            <template v-else-if="downloading">
              <CoButton variant="ghost" @click="closePanel">{{ t("common.close") }}</CoButton>
              <CoButton variant="secondary" :disabled="busy" @click="onCancel">
                {{ t("update.cancel_download") }}
              </CoButton>
            </template>

            <!-- 手动安装包：只能引导，不能自动跑 -->
            <template v-else-if="manualOnly">
              <CoButton variant="ghost" @click="closePanel">{{ t("common.close") }}</CoButton>
              <CoButton variant="secondary" @click="openReleasePage">
                <ExternalLink :size="14" />
                <span>{{ t("update.download_manually") }}</span>
              </CoButton>
            </template>

            <!-- 其余（检查中 / 失败 / 已是最新 / 待自动下载）：重新检查 -->
            <template v-else>
              <CoButton variant="ghost" @click="closePanel">{{ t("common.close") }}</CoButton>
              <CoButton variant="primary" :disabled="busy || checking" @click="onCheck">
                <LoaderCircle v-if="checking" class="update-dialog__spin" :size="14" />
                <RefreshCw v-else :size="14" />
                <span>{{ t("update.check_again") }}</span>
              </CoButton>
            </template>
          </div>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.update-dialog {
  position: fixed;
  inset: 0;
  z-index: 200;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--copper-space-4);
  background: var(--copper-overlay);
  animation: update-dialog-fade var(--copper-duration) var(--copper-easing);
}

.update-dialog__card {
  display: flex;
  flex-direction: column;
  width: min(480px, 100%);
  max-height: calc(100vh - 2 * var(--copper-space-5));
  overflow: hidden;
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  box-shadow: 0 18px 52px rgba(0, 0, 0, 0.45);
  animation: update-dialog-pop var(--copper-duration) var(--copper-easing);
}

.update-dialog__header {
  display: flex;
  align-items: flex-start;
  gap: var(--copper-space-3);
  padding: var(--copper-space-4) var(--copper-space-4) var(--copper-space-3);
}

.update-dialog__icon {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 30px;
  height: 30px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-update-blue);
  color: #ffffff;
}

.update-dialog__icon--downloaded {
  background: color-mix(in srgb, var(--copper-success) 88%, #ffffff);
}

.update-dialog__icon--failed {
  background: color-mix(in srgb, var(--copper-danger) 88%, #ffffff);
}

.update-dialog__heading {
  flex: 1;
  min-width: 0;
}

.update-dialog__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
}

.update-dialog__delta {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
  margin-top: 3px;
  color: var(--copper-text-secondary);
}

.update-dialog__version {
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

.update-dialog__version--old {
  color: var(--copper-text-disabled);
}

.update-dialog__version--new {
  color: var(--copper-accent);
}

.update-dialog__size {
  margin-left: auto;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.update-dialog__close {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 26px;
  height: 26px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: 19px;
  line-height: 1;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.update-dialog__close:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.update-dialog__body {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  padding: 0 var(--copper-space-4) var(--copper-space-3);
  overflow-y: auto;
}

.update-dialog__stats {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--copper-space-3);
}

.update-dialog__figure {
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  font-weight: 600;
}

.update-dialog__dim {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.update-dialog__hint {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  line-height: 1.65;
}

.update-dialog__hint--ready {
  color: var(--copper-text);
}

.update-dialog__hint--dim {
  color: var(--copper-text-disabled);
}

.update-dialog__checksum {
  display: inline-block;
  margin-left: var(--copper-space-2);
  padding: 0 var(--copper-space-2);
  border-radius: var(--copper-radius-sm);
  background: color-mix(in srgb, var(--copper-success) 16%, transparent);
  color: var(--copper-success);
  font-size: var(--copper-font-size-xs);
  font-weight: 600;
}

.update-dialog__error {
  color: var(--copper-danger);
  font-size: var(--copper-font-size-sm);
  line-height: 1.6;
  overflow-wrap: anywhere;
}

.update-dialog__error--inline {
  padding: 0 var(--copper-space-4) var(--copper-space-2);
}

.update-dialog__notes {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding-top: var(--copper-space-3);
  border-top: 1px solid var(--copper-border);
}

.update-dialog__notes-title {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
  font-weight: 600;
  letter-spacing: 0.06em;
  text-transform: uppercase;
}

.update-dialog__notes-body {
  max-height: 132px;
  margin: 0;
  overflow: auto;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  line-height: 1.65;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.update-dialog__footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) var(--copper-space-4);
  background: var(--copper-surface-2);
  border-top: 1px solid var(--copper-border);
}

.update-dialog__link {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-1);
  flex-shrink: 0;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  transition: color var(--copper-duration-fast) var(--copper-easing);
}

.update-dialog__link:hover {
  color: var(--copper-accent);
}

.update-dialog__actions {
  display: flex;
  gap: var(--copper-space-2);
  margin-left: auto;
}

.update-dialog__spin {
  animation: update-dialog-rotate 1s linear infinite;
}

/* 待确认态：轻微左右摆动，提示「再点一次真的执行」。 */
.update-dialog__confirming {
  animation: update-dialog-nudge 1.1s var(--copper-easing) infinite;
}

@keyframes update-dialog-rotate {
  to {
    transform: rotate(360deg);
  }
}

@keyframes update-dialog-nudge {
  0%,
  100% {
    transform: translateX(0);
  }
  25% {
    transform: translateX(-2px);
  }
  75% {
    transform: translateX(2px);
  }
}

@keyframes update-dialog-fade {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

@keyframes update-dialog-pop {
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