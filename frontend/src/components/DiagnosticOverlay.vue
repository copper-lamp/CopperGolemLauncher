<script setup lang="ts">
// 启动诊断面板：把「启动发生了什么」变成用户能看见、能复制的东西。
//
// 设计取舍：
// - 默认不出现。只有引导失败 / 超时，或用户显式用 `?diag=1` 打开时才显示
//   ——正常启动不该被开发信息打扰；
// - 内容是纯文本可复制的报告，而不是一堆只在开发者工具里能看的 console 输出：
//   安卓用户没有 devtools，复制是唯一可行的反馈通道；
// - 面板自身**不依赖任何内核能力**（不走 i18n 命令、不走主题命令），
//   文案取自本地语言包，颜色取自主题令牌：内核挂了它也必须能画出来。

import { computed, ref } from "vue";
import { AlertTriangle, Check, Copy, Info, RefreshCw, X, XCircle, Clock } from "@lucide/vue";

import { bootReport, isBridgeMissing, useBootDiagnostics } from "../boot";
import { diagnosticAutoOpened, diagnosticOpen, setDiagnosticOpen } from "../diag";
import { useI18n } from "../i18n";

const { t } = useI18n();
const open = diagnosticOpen;
const autoOpened = diagnosticAutoOpened;
const { steps, logEntries, ipcState } = useBootDiagnostics();

const copied = ref(false);
const showLog = ref(false);

/** 有失败 / 超时的步骤：面板顶部摘要只讲这些，其余是佐证材料。 */
const failedSteps = computed(() =>
  steps.value.filter((s) => s.state === "failed" || s.state === "timeout"),
);

const stepErrorCount = computed(() => logEntries.value.filter((e) => e.level === "error").length);

/**
 * 宿主 IPC 桥完全缺失是「所有命令都不会有响应」的硬故障。
 *
 * 它的原始错误串（`Cannot read properties of undefined`）对用户毫无意义，
 * 因此单独给一句能照着做的说明，而不是让用户去读堆栈。
 */
const bridgeMissing = isBridgeMissing;

/** 步骤状态文案与图标语义（失败与超时分开：超时更可能是链路问题）。 */
function stepStateKey(state: string): string {
  return `boot.state.${state}`;
}

function stepIcon(state: string) {
  if (state === "ok") return Check;
  if (state === "failed") return XCircle;
  if (state === "timeout") return Clock;
  return Info;
}

function stepCost(startedAt: number, endedAt: number | null): string {
  return endedAt === null ? "…" : `${endedAt - startedAt}ms`;
}

async function copyReport(): Promise<void> {
  const text = bootReport();
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // 剪贴板 API 在部分 WebView / 非安全上下文不可用：退回选中文本让用户手动复制。
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    document.execCommand("copy");
    document.body.removeChild(area);
  }
  copied.value = true;
  window.setTimeout(() => (copied.value = false), 2000);
}

/** 重试：重新加载 WebView。引导失败多半来自一次性的初始化竞态，值得一试。 */
function retry(): void {
  location.reload();
}
</script>

<template>
  <div v-if="open" class="diag">
    <div class="diag__panel">
      <header class="diag__header">
        <AlertTriangle :size="18" class="diag__header-icon" />
        <h2 class="diag__title">
          {{ autoOpened ? t("boot.error.title") : t("boot.diag.title") }}
        </h2>
        <button class="diag__close" type="button" :title="t('common.close')" @click="setDiagnosticOpen(false)">
          <X :size="16" />
        </button>
      </header>

      <p class="diag__lead">
        {{ autoOpened ? t("boot.error.lead") : t("boot.diag.lead") }}
      </p>

      <!-- 失败摘要：一眼看到「哪一步没回来」 -->
      <ul v-if="failedSteps.length > 0" class="diag__failures">
        <li v-for="step in failedSteps" :key="step.id" class="diag__failure">
          <component :is="stepIcon(step.state)" :size="14" class="diag__failure-icon" />
          <span class="diag__failure-name">{{ t(step.labelKey) }}</span>
          <span class="diag__failure-state">{{ t(stepStateKey(step.state)) }}</span>
          <span v-if="step.error" class="diag__failure-error">{{ step.error }}</span>
        </li>
      </ul>

      <!-- 桥缺失是最坏情况：原始堆栈对用户无意义，给一句可照做的说明 -->
      <p v-if="bridgeMissing" class="diag__hint">{{ t("boot.diag.bridge_missing") }}</p>

      <p class="diag__meta">
        {{ t("boot.diag.ipc") }}: <strong>{{ ipcState }}</strong>
        <span class="diag__dot">·</span>
        {{ t("boot.diag.errors") }}: <strong>{{ stepErrorCount }}</strong>
      </p>

      <!-- 全部步骤 -->
      <section class="diag__section">
        <h3 class="diag__section-title">{{ t("boot.diag.steps") }}</h3>
        <ul class="diag__steps">
          <li v-for="step in steps" :key="step.id" class="diag__step" :data-state="step.state">
            <component :is="stepIcon(step.state)" :size="13" class="diag__step-icon" />
            <span class="diag__step-name">{{ t(step.labelKey) }}</span>
            <span class="diag__step-state">{{ t(stepStateKey(step.state)) }}</span>
            <span class="diag__step-cost">{{ stepCost(step.startedAt, step.endedAt) }}</span>
          </li>
        </ul>
      </section>

      <!-- 原始日志（默认折叠：需要时再看） -->
      <section class="diag__section">
        <button class="diag__toggle" type="button" @click="showLog = !showLog">
          {{ showLog ? t("boot.diag.hide_log") : t("boot.diag.show_log") }}
        </button>
        <pre v-if="showLog" class="diag__log">{{ bootReport() }}</pre>
      </section>

      <footer class="diag__actions">
        <button class="diag__btn diag__btn--primary" type="button" @click="retry">
          <RefreshCw :size="15" />
          <span>{{ t("common.retry") }}</span>
        </button>
        <button class="diag__btn" type="button" @click="copyReport">
          <Copy :size="15" />
          <span>{{ copied ? t("common.copied") : t("boot.diag.copy") }}</span>
        </button>
        <button class="diag__btn" type="button" @click="setDiagnosticOpen(false)">
          <span>{{ t("boot.diag.dismiss") }}</span>
        </button>
      </footer>
    </div>
  </div>
</template>

<style scoped>
.diag {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--copper-space-4);
  background: var(--copper-overlay);
  overflow-y: auto;
}

.diag__panel {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  width: min(680px, 100%);
  max-height: 100%;
  padding: var(--copper-space-4);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  box-shadow: var(--copper-shadow);
  overflow-y: auto;
}

.diag__header {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.diag__header-icon {
  flex-shrink: 0;
  color: var(--copper-warning);
}

.diag__title {
  flex: 1;
  min-width: 0;
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
  color: var(--copper-text);
}

.diag__close {
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
}

.diag__close:active {
  background: var(--copper-active);
}

.diag__lead {
  color: var(--copper-text-secondary);
  line-height: 1.6;
}

.diag__failures {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
  padding: var(--copper-space-2) var(--copper-space-3);
  list-style: none;
  background: color-mix(in srgb, var(--copper-danger) 12%, transparent);
  border: 1px solid color-mix(in srgb, var(--copper-danger) 40%, transparent);
  border-radius: var(--copper-radius-md);
}

.diag__failure {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--copper-space-2);
  font-size: var(--copper-font-size-sm);
}

.diag__failure-icon {
  flex-shrink: 0;
  color: var(--copper-danger);
}

.diag__failure-name {
  color: var(--copper-text);
  font-weight: 500;
}

.diag__failure-state {
  color: var(--copper-danger);
}

.diag__failure-error {
  flex-basis: 100%;
  color: var(--copper-text-secondary);
  word-break: break-word;
}

.diag__hint {
  padding: var(--copper-space-2) var(--copper-space-3);
  background: color-mix(in srgb, var(--copper-warning) 14%, transparent);
  border: 1px solid color-mix(in srgb, var(--copper-warning) 40%, transparent);
  border-radius: var(--copper-radius-md);
  color: var(--copper-text);
  font-size: var(--copper-font-size-sm);
  line-height: 1.6;
}

.diag__meta {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.diag__dot {
  margin: 0 var(--copper-space-1);
}

.diag__section {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.diag__section-title {
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
  color: var(--copper-text-secondary);
}

.diag__steps {
  display: flex;
  flex-direction: column;
  gap: 2px;
  list-style: none;
  font-size: var(--copper-font-size-sm);
}

.diag__step {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  padding: var(--copper-space-1) var(--copper-space-2);
  border-radius: var(--copper-radius-sm);
  background: var(--copper-surface-2);
}

.diag__step-icon {
  flex-shrink: 0;
  color: var(--copper-text-disabled);
}

.diag__step[data-state="ok"] .diag__step-icon {
  color: var(--copper-success);
}

.diag__step[data-state="failed"] .diag__step-icon {
  color: var(--copper-danger);
}

.diag__step[data-state="timeout"] .diag__step-icon {
  color: var(--copper-warning);
}

.diag__step-name {
  flex: 1;
  min-width: 0;
  color: var(--copper-text);
}

.diag__step-state {
  color: var(--copper-text-secondary);
}

.diag__step-cost {
  min-width: 64px;
  text-align: right;
  color: var(--copper-text-disabled);
  font-variant-numeric: tabular-nums;
}

.diag__toggle {
  align-self: flex-start;
  padding: var(--copper-space-1) 0;
  border: none;
  background: transparent;
  color: var(--copper-info);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
}

.diag__log {
  max-height: 220px;
  padding: var(--copper-space-2);
  overflow: auto;
  background: var(--copper-bg);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-sm);
  color: var(--copper-text-secondary);
  font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  font-size: var(--copper-font-size-xs);
  white-space: pre-wrap;
  word-break: break-word;
  user-select: text;
}

.diag__actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--copper-space-2);
}

.diag__btn {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
  min-height: 36px;
  padding: 0 var(--copper-space-4);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.diag__btn:active {
  background: var(--copper-active);
}

.diag__btn--primary {
  border-color: var(--copper-accent);
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
}
</style>
