<script setup lang="ts">
// 更新面板：版本对照 + 发行说明 + 进度 + 动作按钮。
//
// 面板是更新流程里唯一承载「发生了什么 / 现在到哪一步 / 下一步是什么」的地方，
// 因此每个阶段都必须有明确的视觉状态与文案，不能出现「点了没反应」或
// 「进度条卡在 0% 却什么都不说」：
//
// | 阶段        | 主视觉              | 主动作              |
// | ----------- | ------------------- | ------------------- |
// | checking    | 不确定态进度 + 转圈 | 检查中（禁用）       |
// | available   | 版本对照 + 体积     | 下载更新             |
// | downloading | 进度条 + 速度 + ETA | 取消下载             |
// | downloaded  | 成功态进度 + 校验号 | 重启并更新           |
// | failed      | 错误原文            | 重试（按失败来源）    |
// | idle        | 已是最新            | 重新检查             |
import { computed, ref } from "vue";
import {
  AlertTriangle,
  ArrowRight,
  CheckCircle2,
  Download,
  ExternalLink,
  LoaderCircle,
  RefreshCw,
  Sparkles,
  X,
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
  closePanel,
  check,
  download,
  cancel,
  install,
} = useUpdate();

/** 动作失败原因（面板内展示，不弹吐司：吐司会遮住用户正在看的内容）。 */
const actionError = ref<string | null>(null);

/** 是否为手动安装包（需用户自行到发行页下载）。 */
const manualOnly = computed(() => latest.value != null && !autoInstallable.value);

const progressRatio = computed(() => progress.value?.ratio ?? null);

const checking = computed(() => phase.value === "checking");
const downloading = computed(() => phase.value === "downloading");
const downloaded = computed(() => phase.value === "downloaded");
const available = computed(() => phase.value === "available");

/** 发行说明是否为空。 */
const hasNotes = computed(() => (latest.value?.notes ?? "").trim().length > 0);

function describe(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** 手动检查：失败原因留在面板内（`error` 由状态派生，这里只兜住 IPC 层异常）。 */
async function onCheck() {
  actionError.value = null;
  try {
    await check();
  } catch (e) {
    actionError.value = describe(e);
  }
}

/** 投递下载。 */
async function onDownload() {
  actionError.value = null;
  try {
    await download();
  } catch (e) {
    actionError.value = describe(e);
    showToast(describe(e), "error");
  }
}

/** 取消下载。 */
async function onCancel() {
  actionError.value = null;
  try {
    await cancel();
  } catch (e) {
    actionError.value = describe(e);
  }
}

/** 确认重启更新（这是唯一有不可逆后果的动作，必须二次确认）。 */
const confirmInstall = ref(false);

async function onInstall() {
  if (!confirmInstall.value) {
    confirmInstall.value = true;
    return;
  }
  actionError.value = null;
  await install();
  confirmInstall.value = false;
}

async function openReleasePage() {
  const url = latest.value?.html_url;
  if (!url) return;
  await openExternal(url).catch(() => {
    showToast(t("update.open_page_failed"), "error");
  });
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="status"
      class="update-panel"
      role="dialog"
      aria-modal="true"
      @click.self="closePanel"
    >
      <div class="update-panel__card">
        <header class="update-panel__header">
          <div class="update-panel__heading">
            <span class="update-panel__icon" :class="`update-panel__icon--${phase}`">
              <Sparkles v-if="phase === 'available'" :size="15" />
              <CheckCircle2 v-else-if="phase === 'downloaded'" :size="15" />
              <AlertTriangle v-else-if="phase === 'failed'" :size="15" />
              <Download v-else :size="15" />
            </span>
            <h2 class="update-panel__title">
              <template v-if="phase === 'available'">{{ t("update.available_title") }}</template>
              <template v-else-if="phase === 'downloaded'">{{ t("update.ready_title") }}</template>
              <template v-else-if="phase === 'failed'">{{ t("update.failed_title") }}</template>
              <template v-else-if="checking">{{ t("update.checking_title") }}</template>
              <template v-else-if="downloading">{{ t("update.downloading_title") }}</template>
              <template v-else>{{ t("update.idle_title") }}</template>
            </h2>
          </div>
          <button class="update-panel__close" :title="t('common.close')" @click="closePanel">
            <X :size="16" />
          </button>
        </header>

        <!-- 版本对照：只在真的有新旧之分时出现，避免"0.1.0 → 0.1.0"这种噪音 -->
        <div v-if="versionDelta" class="update-panel__delta">
          <span class="update-panel__version update-panel__version--old">
            v{{ versionDelta.current }}
          </span>
          <ArrowRight :size="14" class="update-panel__delta-arrow" />
          <span class="update-panel__version update-panel__version--new">
            v{{ versionDelta.next }}
          </span>
          <span v-if="latest?.asset_size" class="update-panel__size">
            {{ formatBytes(latest.asset_size) }}
          </span>
        </div>

        <!-- 检查中：不确定态进度 + 转圈，明确告知"在等网络" -->
        <div v-if="checking" class="update-panel__body">
          <p class="update-panel__hint">{{ t("update.checking_hint") }}</p>
          <CoProgress :value="null" />
        </div>

        <!-- 下载中：进度条 + 字节 + 速度 + ETA + 校验状态 -->
        <div v-else-if="downloading" class="update-panel__body">
          <div class="update-panel__stats">
            <span class="update-panel__progress-text">
              <template v-if="progress?.total">
                {{ formatBytes(progress.downloaded) }} / {{ formatBytes(progress.total) }}
              </template>
              <template v-else>{{ formatBytes(progress?.downloaded ?? 0) }}</template>
            </span>
            <span v-if="progress?.speed" class="update-panel__meta">
              {{ formatSpeed(progress.speed) }}
              <template v-if="progress.etaSeconds != null">
                · {{ t("update.eta", { seconds: formatEta(progress.etaSeconds) }) }}
              </template>
            </span>
          </div>
          <CoProgress :value="progressRatio" />
          <p class="update-panel__hint">{{ t("update.downloading_hint") }}</p>
        </div>

        <!-- 已就绪：成功态进度 + 校验信息 -->
        <div v-else-if="downloaded" class="update-panel__body">
          <CoProgress :value="1" tone="success" />
          <div class="update-panel__stats">
            <span class="update-panel__progress-text">{{ t("update.ready_body") }}</span>
            <span v-if="latest?.sha256" class="update-panel__meta update-panel__checksum">
              {{ t("update.checksum_ok") }}
            </span>
          </div>
        </div>

        <!-- 失败：原文展示 + 按失败来源给不同的恢复动作 -->
        <div v-else-if="phase === 'failed'" class="update-panel__body">
          <p class="update-panel__error">{{ error ?? t("update.failed_generic") }}</p>
          <p class="update-panel__hint">
            <template v-if="errorKind === 'no_asset'">{{ t("update.failed_no_asset_hint") }}</template>
            <template v-else-if="errorKind === 'download'">{{ t("update.failed_download_hint") }}</template>
            <template v-else>{{ t("update.failed_network_hint") }}</template>
          </p>
          <!-- 下载失败时把已下字节摆出来：用户能据此判断是从头再来还是几乎完成。 -->
          <div v-if="errorKind === 'download' && progress" class="update-panel__stats">
            <span class="update-panel__progress-text">
              {{ formatBytes(progress.downloaded) }}
              <template v-if="progress.total">/ {{ formatBytes(progress.total) }}</template>
            </span>
          </div>
        </div>

        <!-- 已是最新 -->
        <div v-else class="update-panel__body">
          <p class="update-panel__hint">
            {{ t("update.up_to_date_body", { version: status.current_version }) }}
          </p>
          <p v-if="status.last_checked_at" class="update-panel__hint update-panel__hint--dim">
            {{ t("update.checked_at", { time: formatTime(status.last_checked_at) }) }}
          </p>
        </div>

        <!-- 发行说明 -->
        <div v-if="hasNotes" class="update-panel__notes">
          <h3 class="update-panel__notes-title">{{ t("update.notes_title") }}</h3>
          <pre class="update-panel__notes-body">{{ latest?.notes }}</pre>
        </div>

        <p v-if="actionError" class="update-panel__error update-panel__error--inline">
          {{ actionError }}
        </p>

        <footer class="update-panel__footer">
          <button
            v-if="latest?.html_url"
            class="update-panel__link"
            type="button"
            @click="openReleasePage"
          >
            <ExternalLink :size="13" />
            <span>{{ t("update.view_release") }}</span>
          </button>
          <div class="update-panel__actions">
            <!-- 有新版本：等待用户决定 -->
            <template v-if="available && latest">
              <CoButton variant="ghost" @click="closePanel">{{ t("update.later") }}</CoButton>
              <CoButton
                v-if="autoInstallable"
                variant="primary"
                :disabled="busy"
                @click="onDownload"
              >
                <Download :size="14" />
                <span>{{ t("update.download_now") }}</span>
              </CoButton>
              <CoButton v-else variant="secondary" @click="openReleasePage">
                <ExternalLink :size="14" />
                <span>{{ t("update.download_manually") }}</span>
              </CoButton>
            </template>
            <!-- 手动安装包：只能提示，不能自动装 -->
            <template v-else-if="manualOnly && !downloading && !downloaded">
              <CoButton variant="ghost" @click="closePanel">{{ t("common.close") }}</CoButton>
              <CoButton variant="secondary" @click="openReleasePage">
                <ExternalLink :size="14" />
                <span>{{ t("update.download_manually") }}</span>
              </CoButton>
            </template>
            <!-- 下载中：可取消 -->
            <template v-else-if="downloading">
              <CoButton variant="ghost" :disabled="busy" @click="onCancel">
                {{ t("update.cancel_download") }}
              </CoButton>
            </template>
            <!-- 已就绪：重启并更新（确认式二次点击） -->
            <template v-else-if="downloaded">
              <CoButton variant="ghost" @click="closePanel">{{ t("update.later") }}</CoButton>
              <CoButton
                variant="primary"
                :disabled="busy"
                :class="{ 'update-panel__restart-confirm': confirmInstall }"
                @click="onInstall"
              >
                <LoaderCircle v-if="busy" class="update-panel__btn-spin" :size="14" />
                <RefreshCw v-else :size="14" />
                <span>{{ confirmInstall ? t("update.confirm_restart") : t("update.restart_now") }}</span>
              </CoButton>
            </template>
            <!-- 失败 / 空闲：重试或重新检查 -->
            <template v-else>
              <CoButton variant="ghost" @click="closePanel">{{ t("common.close") }}</CoButton>
              <CoButton variant="primary" :disabled="busy" @click="onCheck">
                <LoaderCircle v-if="checking" class="update-panel__btn-spin" :size="14" />
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
.update-panel {
  position: fixed;
  inset: 0;
  z-index: 200;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--copper-overlay);
  animation: update-panel-fade var(--copper-duration) var(--copper-easing);
}

.update-panel__card {
  display: flex;
  flex-direction: column;
  width: min(520px, calc(100vw - 48px));
  max-height: calc(100vh - 96px);
  overflow: hidden;
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.4);
  animation: update-panel-pop var(--copper-duration) var(--copper-easing);
}

.update-panel__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--copper-space-4) var(--copper-space-4) var(--copper-space-3);
}

.update-panel__heading {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.update-panel__icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border-radius: var(--copper-radius-sm);
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
  color: var(--copper-accent);
}

.update-panel__icon--downloaded {
  background: color-mix(in srgb, var(--copper-success) 14%, transparent);
  color: var(--copper-success);
}

.update-panel__icon--failed {
  background: color-mix(in srgb, var(--copper-danger) 14%, transparent);
  color: var(--copper-danger);
}

.update-panel__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
}

.update-panel__close {
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
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.update-panel__close:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.update-panel__delta {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  margin: 0 var(--copper-space-4) var(--copper-space-3);
  padding: var(--copper-space-2) var(--copper-space-3);
  border-radius: var(--copper-radius-sm);
  background: var(--copper-surface-2);
}

.update-panel__version {
  font-size: var(--copper-font-size-md);
  font-weight: 700;
}

.update-panel__version--old {
  color: var(--copper-text-disabled);
}

.update-panel__version--new {
  color: var(--copper-accent);
}

.update-panel__delta-arrow {
  color: var(--copper-text-disabled);
}

.update-panel__size {
  margin-left: auto;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.update-panel__body {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  padding: 0 var(--copper-space-4) var(--copper-space-3);
}

.update-panel__stats {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--copper-space-3);
  font-size: var(--copper-font-size-sm);
}

.update-panel__progress-text {
  color: var(--copper-text);
  font-weight: 600;
}

.update-panel__meta {
  color: var(--copper-text-secondary);
}

.update-panel__checksum {
  color: var(--copper-success);
}

.update-panel__hint {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  line-height: 1.6;
}

.update-panel__hint--dim {
  color: var(--copper-text-disabled);
}

.update-panel__error {
  color: var(--copper-danger);
  font-size: var(--copper-font-size-sm);
  line-height: 1.6;
  overflow-wrap: anywhere;
}

.update-panel__error--inline {
  padding: 0 var(--copper-space-4);
}

.update-panel__notes {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4);
  border-top: 1px solid var(--copper-border);
}

.update-panel__notes-title {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.06em;
}

.update-panel__notes-body {
  max-height: 140px;
  margin: 0;
  overflow: auto;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  line-height: 1.65;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.update-panel__footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-3) var(--copper-space-4);
  background: var(--copper-surface-2);
  border-top: 1px solid var(--copper-border);
}

.update-panel__link {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-1);
  padding: 0;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  transition: color var(--copper-duration-fast) var(--copper-easing);
}

.update-panel__link:hover {
  color: var(--copper-accent);
}

.update-panel__actions {
  display: flex;
  gap: var(--copper-space-2);
  margin-left: auto;
}

.update-panel__btn-spin {
  animation: update-panel-spin 1s linear infinite;
}

/* 二次确认态：按钮转为「点击确认」的强调外观，避免误触重启。 */
.update-panel__restart-confirm {
  animation: update-panel-nudge 1.1s var(--copper-easing) infinite;
}

@keyframes update-panel-spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes update-panel-nudge {
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

@keyframes update-panel-fade {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

@keyframes update-panel-pop {
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