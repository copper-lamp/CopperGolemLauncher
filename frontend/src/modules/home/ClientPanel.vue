<script setup lang="ts">
// 客户端页右侧面板：安装入口 + 当前客户端详情 + 注入设置。
//
// 搜索框不在这里 —— 它属于页面的右栏顶部（与资源页 / 模组页同一套版面），
// 面板只负责「导入、看详情、调注入时机、删除」这些和当前客户端相关的动作。
//
// 拖拽用 Tauri 的 webview 拖放事件而不是 HTML5 `drop`：WebView2 下
// `dataTransfer.files` 拿不到磁盘路径，而导入需要的是路径。

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import {
  Check,
  FileWarning,
  Loader2,
  PlugZap,
  Trash2,
} from "@lucide/vue";
import { getCurrentWebview } from "@tauri-apps/api/webview";

import { useI18n } from "../../i18n";
import CoButton from "../../components/ui/CoButton.vue";
import type { ClientView } from "../../api/home";

const props = defineProps<{
  /** 当前实例选中的客户端；未选择时为 null。 */
  client: ClientView | null;
  /** 当前实例是否处于客户端模式。 */
  clientMode: boolean;
  importing: boolean;
  saving: boolean;
  /** 已选好文件、等待命名的导入路径（空表示没有待确认的导入）。 */
  importPath: string;
  importName: string;
  importError: string;
}>();

const emit = defineEmits<{
  pickFile: [];
  importDropped: [path: string];
  rejectDrop: [];
  confirmImport: [];
  cancelImport: [];
  "update:importName": [value: string];
  delete: [];
  enableMode: [];
  setDelay: [seconds: number];
}>();

const { t } = useI18n();

const zone = ref<HTMLElement | null>(null);
const dragging = ref(false);
const delayDraft = ref("10");
const delayError = ref("");

const delaySeconds = computed(() => Math.round((props.client?.inject_delay_ms ?? 10000) / 1000));

watch(
  () => props.client,
  (client) => {
    delayDraft.value = String(Math.round((client?.inject_delay_ms ?? 10000) / 1000));
    delayError.value = "";
  },
  { immediate: true },
);

function commitDelay() {
  if (!props.client || props.saving) return;
  const seconds = Number(delayDraft.value);
  if (!Number.isFinite(seconds) || seconds < 1 || seconds > 120) {
    delayError.value = t("module.home.client.inject_range");
    delayDraft.value = String(delaySeconds.value);
    return;
  }
  const rounded = Math.round(seconds);
  if (rounded * 1000 === props.client.inject_delay_ms) {
    delayDraft.value = String(rounded);
    return;
  }
  delayError.value = "";
  emit("setDelay", rounded);
}

/** 指针是否落在虚线框内（拖放事件给的是物理像素）。 */
function insideZone(position?: { x: number; y: number }): boolean {
  const el = zone.value;
  if (!el || !position) return false;
  const rect = el.getBoundingClientRect();
  const ratio = window.devicePixelRatio || 1;
  const x = position.x / ratio;
  const y = position.y / ratio;
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

let disposeDrag: (() => void) | null = null;

onMounted(async () => {
  try {
    disposeDrag = await getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "enter" || payload.type === "over") {
        dragging.value = insideZone(payload.position);
        return;
      }
      if (payload.type === "drop") {
        const inside = insideZone(payload.position);
        dragging.value = false;
        if (!inside) return;
        const dll = payload.paths.find((path) => path.toLowerCase().endsWith(".dll"));
        if (dll) emit("importDropped", dll);
        else emit("rejectDrop");
        return;
      }
      dragging.value = false;
    });
  } catch {
    // 纯浏览器调试没有 webview 拖放：导入退化为点击选择文件。
    disposeDrag = null;
  }
});

onUnmounted(() => {
  disposeDrag?.();
  disposeDrag = null;
});

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 MB";
  return (bytes / 1024 / 1024).toFixed(1) + " MB";
}

function formatDate(millis: number): string {
  if (!millis) return "";
  const date = new Date(millis);
  const pad = (value: number) => String(value).padStart(2, "0");
  return date.getFullYear() + "." + pad(date.getMonth() + 1) + "." + pad(date.getDate());
}
</script>

<template>
  <div class="panel">
    <div
      ref="zone"
      :class="['panel__drop', { 'panel__drop--active': dragging, 'panel__drop--busy': importing }]"
      role="button"
      tabindex="0"
      @click="emit('pickFile')"
      @keydown.enter.prevent="emit('pickFile')"
    >
      <Loader2 v-if="importing" :size="20" class="panel__spin" :stroke-width="1.6" />
      <PlugZap v-else :size="20" :stroke-width="1.6" />
      <span class="panel__drop-title">
        {{ importing ? t("module.home.client.importing") : t("module.home.client.drop_title") }}
      </span>
      <span class="panel__drop-hint">{{ t("module.home.client.drop_hint") }}</span>
    </div>

    <section v-if="importPath" class="panel__import">
      <h3 class="panel__import-title">{{ t("module.home.client.import_title") }}</h3>
      <p class="panel__import-path">{{ importPath }}</p>
      <label class="panel__field">
        <span class="panel__field-label">{{ t("module.home.client.name_label") }}</span>
        <input
          :value="importName"
          class="panel__input"
          :placeholder="t('module.home.client.name_placeholder')"
          :disabled="importing"
          @input="emit('update:importName', ($event.target as HTMLInputElement).value)"
          @keydown.enter="emit('confirmImport')"
        />
        <span class="panel__field-hint">{{ t("module.home.client.name_hint") }}</span>
      </label>
      <p v-if="importError" class="panel__error">{{ importError }}</p>
      <div class="panel__import-actions">
        <CoButton variant="secondary" size="sm" :disabled="importing" @click="emit('cancelImport')">
          {{ t("module.home.client.cancel") }}
        </CoButton>
        <CoButton variant="primary" size="sm" :disabled="importing" @click="emit('confirmImport')">
          {{ t("module.home.client.import_confirm") }}
        </CoButton>
      </div>
    </section>

    <template v-else-if="client">
      <section class="panel__client">
        <header class="panel__client-head">
          <span class="panel__client-icon">
            <PlugZap :size="18" :stroke-width="1.7" />
          </span>
          <div class="panel__client-text">
            <span class="panel__client-name">{{ client.name }}</span>
            <span class="panel__client-meta">
              {{ client.file_name }} · {{ formatBytes(client.size) }}
              <template v-if="client.imported_at"> · {{ formatDate(client.imported_at) }}</template>
            </span>
          </div>
          <span
            :class="['panel__badge', { 'panel__badge--on': clientMode }]"
            :title="clientMode ? t('module.home.client.mode_on') : t('module.home.client.mode_off')"
          >
            <Check v-if="clientMode" :size="12" />
            {{ clientMode ? t("module.home.client.mode_on_short") : t("module.home.client.mode_off_short") }}
          </span>
        </header>

        <p v-if="!client.available" class="panel__warning">
          <FileWarning :size="14" />
          <span>{{ t("module.home.client.missing") }}</span>
        </p>
        <div v-else-if="!clientMode" class="panel__warning">
          <span>{{ t("module.home.client.not_client_mode") }}</span>
          <CoButton variant="secondary" size="sm" @click="emit('enableMode')">
            {{ t("module.home.client.enable_mode") }}
          </CoButton>
        </div>

        <p class="panel__path" :title="client.path">{{ client.path }}</p>
      </section>

      <section class="panel__section">
        <header class="panel__section-head">
          <span class="panel__section-title">{{ t("module.home.client.inject_title") }}</span>
        </header>
        <p class="panel__section-hint">
          {{ t("module.home.client.inject_hint", { seconds: delaySeconds }) }}
        </p>
        <div class="panel__delay">
          <label class="panel__delay-field">
            <input
              v-model="delayDraft"
              class="panel__input panel__input--number"
              type="number"
              min="1"
              max="120"
              step="1"
              :disabled="saving"
              @blur="commitDelay"
              @keydown.enter="commitDelay"
            />
            <span class="panel__delay-unit">{{ t("module.home.client.inject_unit") }}</span>
          </label>
          <span v-if="delayError" class="panel__error">{{ delayError }}</span>
        </div>
      </section>

      <div class="panel__footer">
        <CoButton variant="danger" size="sm" :disabled="saving" @click="emit('delete')">
          <Trash2 :size="14" />
          <span>{{ t("module.home.client.delete") }}</span>
        </CoButton>
      </div>
    </template>

    <p v-else class="panel__empty">{{ t("module.home.client.panel_empty") }}</p>
  </div>
</template>

<style scoped>
.panel {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-4);
  padding: var(--copper-space-4);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow-subtle);
}

/* 虚线框：安装快捷方式。虚线本身就是「可以扔进来」的提示，不用实线描边。 */
.panel__drop {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-1);
  padding: var(--copper-space-5) var(--copper-space-3);
  border: 1px dashed color-mix(in srgb, var(--copper-border) 90%, transparent);
  border-radius: var(--copper-radius-md);
  color: var(--copper-text-secondary);
  text-align: center;
  cursor: pointer;
  outline: none;
  transition:
    border-color var(--copper-duration-fast) var(--copper-easing),
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.panel__drop:hover {
  border-color: color-mix(in srgb, var(--copper-accent) 55%, var(--copper-border));
  color: var(--copper-text);
}

.panel__drop:focus-visible {
  border-color: color-mix(in srgb, var(--copper-accent) 70%, transparent);
}

.panel__drop--active {
  border-color: var(--copper-accent);
  background: color-mix(in srgb, var(--copper-accent) 10%, transparent);
  color: var(--copper-text);
}

.panel__drop--busy {
  cursor: progress;
}

.panel__drop-title {
  font-size: var(--copper-font-size-md);
  font-weight: 500;
}

.panel__drop-hint {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.panel__import {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  animation: panel-in var(--copper-duration) var(--copper-easing);
}

.panel__import-title {
  margin: 0;
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

.panel__import-path {
  margin: 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  overflow-wrap: anywhere;
}

.panel__field {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.panel__field-label {
  font-size: var(--copper-font-size-sm);
}

.panel__field-hint {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  line-height: 1.5;
}

.panel__input {
  width: 100%;
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
}

.panel__input:focus-visible {
  outline: none;
  border-color: var(--copper-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--copper-accent) 22%, transparent);
}

.panel__input:disabled {
  opacity: 0.5;
}

.panel__import-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--copper-space-2);
}

.panel__error {
  margin: 0;
  color: var(--copper-danger);
  font-size: var(--copper-font-size-xs);
  line-height: 1.5;
}

.panel__client {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
}

.panel__client-head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  min-width: 0;
}

.panel__client-icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  border-radius: var(--copper-radius-md);
  background: color-mix(in srgb, var(--copper-accent) 16%, transparent);
  color: var(--copper-accent);
}

.panel__client-text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.panel__client-name {
  overflow: hidden;
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.panel__client-meta {
  overflow: hidden;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
  text-overflow: ellipsis;
}

.panel__badge {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 2px 8px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-3);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.panel__badge--on {
  background: color-mix(in srgb, var(--copper-accent) 18%, transparent);
  color: var(--copper-accent);
}

.panel__warning {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-2);
  margin: 0;
  padding: var(--copper-space-2);
  border-radius: var(--copper-radius-sm);
  background: color-mix(in srgb, var(--copper-warning) 12%, transparent);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  line-height: 1.5;
}

.panel__path {
  margin: 0;
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
  overflow-wrap: anywhere;
}

.panel__section {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding-top: var(--copper-space-2);
}

.panel__section-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.panel__section-title {
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

.panel__section-hint {
  margin: 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  line-height: 1.6;
}

.panel__delay {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.panel__delay-field {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.panel__input--number {
  width: 88px;
  text-align: center;
  font-variant-numeric: tabular-nums;
}

.panel__delay-unit {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.panel__footer {
  margin-top: auto;
  padding-top: var(--copper-space-2);
  display: flex;
  justify-content: flex-end;
}

.panel__empty {
  margin: 0;
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
  line-height: 1.6;
}

.panel__spin {
  animation: panel-spin 900ms linear infinite;
}

@keyframes panel-spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes panel-in {
  from {
    opacity: 0;
    transform: translateY(4px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}
</style>
