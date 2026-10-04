<script setup lang="ts">
// 安装确认弹窗：给这次安装的实例命名，确认后投递安装。
//
// 规则来自需求：**不能有同名实例**。判定完全在后端（`game_download_instance_check`），
// 前端只负责当场把结果翻译成一句人话——两侧各写一套校验规则必然会漂移，而漂移的
// 后果是「目录建出来了，宿主按另一个名字去找」。
//
// 同一个版本可以被安装很多次：每次确认都会新建一个实例，实例之间互相隔离。因此弹窗
// 的初值取后端推荐的可用名（版本号，重名时自动加序号），而不是让用户自己撞名字。

import { computed, ref, watch } from "vue";
import { LoaderCircle, X } from "@lucide/vue";

import CoButton from "../../components/ui/CoButton.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { useI18n } from "../../i18n";
import type { GameVersionView } from "./api";
import { useGameDownload } from "./useGameDownload";

const props = defineProps<{
  open: boolean;
  version: GameVersionView | null;
  loader: string | null;
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
}>();

const { t } = useI18n();
const gd = useGameDownload();

const MB_KEY = "module.game-download";

const name = ref("");
/** 校验结果：`null` 表示尚未校验（输入中）/ 无法确认。 */
const available = ref<boolean | null>(null);
const reason = ref<string | null>(null);
const checking = ref(false);
const submitting = ref(false);

/** 校验请求序号：只认最后一次输入的结果，避免快速输入时旧响应覆盖新状态。 */
let checkToken = 0;
let debounce = 0;

/** 弹窗标题里的版本号（`version` 可能尚未加载）。 */
const title = computed(() =>
  props.version ? props.version.game_version : t(`${MB_KEY}.detailTitle`),
);

/** 可以提交：名字非空、已确认可用、且没有正在进行的提交。 */
const canSubmit = computed(
  () => !submitting.value && available.value === true && name.value.trim().length > 0,
);

async function runCheck(raw: string) {
  const token = ++checkToken;
  if (!raw.trim()) {
    available.value = false;
    reason.value = "empty";
    return;
  }
  checking.value = true;
  const result = await gd.checkInstance(raw);
  // 过期的响应直接丢弃。
  if (token !== checkToken) return;
  checking.value = false;
  if (!result) {
    // 查询失败：宁可不让提交，也不能放行一个可能重名的实例。
    available.value = null;
    reason.value = "unknown";
    return;
  }
  available.value = result.available;
  reason.value = result.reason;
  // 后端会规整名字（非法字符收敛为 `_`），把规整结果显示给用户，
  // 避免「我明明输入的是 A，目录里却是 B」。
  if (result.name !== raw && result.name !== name.value) {
    name.value = result.name;
  }
}

function onInput(value: string) {
  name.value = value;
  available.value = null;
  reason.value = null;
  window.clearTimeout(debounce);
  debounce = window.setTimeout(() => void runCheck(value), 220);
}

/** 每次打开都重新取推荐名并校验（上一次提交可能已经占用了那个名字）。 */
watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    submitting.value = false;
    name.value = "";
    available.value = null;
    reason.value = null;
    if (!props.version) return;
    const suggested = await gd.suggestInstance(
      props.version.id,
      props.version.game_version,
      props.loader,
    );
    name.value = suggested;
    await runCheck(suggested);
  },
);

function close() {
  if (submitting.value) return;
  emit("update:open", false);
}

async function confirm() {
  if (!canSubmit.value || !props.version) return;
  submitting.value = true;
  const ok = await gd.install(props.version.id, name.value.trim(), props.loader);
  submitting.value = false;
  if (ok) emit("update:open", false);
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="open && version"
      class="name-dialog"
      role="dialog"
      aria-modal="true"
      @click.self="close"
    >
      <div class="name-dialog__card">
        <header class="name-dialog__header">
          <h2 class="name-dialog__title">{{ t(`${MB_KEY}.dialog.title`) }}</h2>
          <button class="name-dialog__close" :title="t('common.close')" @click="close">
            <X :size="16" />
          </button>
        </header>

        <div class="name-dialog__body">
          <p class="name-dialog__version">{{ title }}</p>
          <p class="name-dialog__intro">{{ t(`${MB_KEY}.dialog.intro`) }}</p>

          <label class="name-dialog__field">
            <span class="name-dialog__label">{{ t(`${MB_KEY}.dialog.field_name`) }}</span>
            <CoTextField
              :model-value="name"
              :placeholder="t(`${MB_KEY}.dialog.name_placeholder`)"
              :disabled="submitting"
              @update:modelValue="onInput"
              @enter="confirm"
            />
          </label>

          <p v-if="checking" class="name-dialog__hint">
            <LoaderCircle :size="12" class="spin" />
            {{ t(`${MB_KEY}.dialog.checking`) }}
          </p>
          <p v-else-if="reason" class="name-dialog__hint name-dialog__hint--error">
            {{ t(`${MB_KEY}.dialog.reason.${reason}`) }}
          </p>
          <p v-else-if="available" class="name-dialog__hint name-dialog__hint--ok">
            {{ t(`${MB_KEY}.dialog.available`) }}
          </p>
        </div>

        <footer class="name-dialog__footer">
          <CoButton variant="ghost" :disabled="submitting" @click="close">
            {{ t("common.cancel") }}
          </CoButton>
          <CoButton variant="primary" :disabled="!canSubmit" @click="confirm">
            <LoaderCircle v-if="submitting" :size="14" class="spin" />
            {{ t(`${MB_KEY}.dialog.confirm`) }}
          </CoButton>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.name-dialog {
  position: fixed;
  inset: 0;
  z-index: 100;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--copper-overlay);
  animation: name-dialog-fade var(--copper-duration) var(--copper-easing);
}

.name-dialog__card {
  width: min(420px, calc(100vw - 48px));
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  box-shadow: var(--copper-shadow);
  animation: name-dialog-pop var(--copper-duration) var(--copper-easing);
  overflow: hidden;
}

.name-dialog__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--copper-space-4) var(--copper-space-4) 0;
}

.name-dialog__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
}

.name-dialog__close {
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

.name-dialog__close:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.name-dialog__body {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding: var(--copper-space-4);
}

.name-dialog__version {
  margin: 0;
  font-size: var(--copper-font-size-md);
  font-weight: 600;
}

.name-dialog__intro {
  margin: 0;
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.name-dialog__field {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
  min-width: 0;
}

.name-dialog__label {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.name-dialog__hint {
  display: flex;
  align-items: center;
  gap: 5px;
  margin: 0;
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.name-dialog__hint--error {
  color: var(--copper-danger);
}

.name-dialog__hint--ok {
  color: var(--copper-success);
}

.name-dialog__footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4);
  background: var(--copper-surface-2);
}

.spin {
  animation: spin 1.2s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes name-dialog-fade {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

@keyframes name-dialog-pop {
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
