<script setup lang="ts">
// 版本封面编辑弹窗：上传自定义图片（canvas 中心裁剪 256×256 方形 PNG）+ 移除封面。
// 官方预设图标需内置资源包，当前未接入（见设计文档 TODO）。

import { ref, watch } from "vue";
import { X, Upload, Trash2, Image as ImageIcon } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoButton from "../../components/ui/CoButton.vue";
import { homeLogoRemove, homeLogoSet } from "../../api/home";

const props = defineProps<{
  open: boolean;
  versionName: string;
  currentLogo: string | null;
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
  saved: [];
}>();

const { t } = useI18n();

const preview = ref<string | null>(null);
const saving = ref(false);
const fileInput = ref<HTMLInputElement | null>(null);

// 每次打开重置预览。
watch(
  () => props.open,
  (open) => {
    if (open) preview.value = null;
  },
);

function close() {
  if (saving.value) return;
  emit("update:open", false);
}

function pickFile() {
  fileInput.value?.click();
}

async function onFileChange(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  try {
    const dataUrl = await readFile(file);
    const img = await loadImage(dataUrl);
    const size = Math.min(img.width, img.height);
    const canvas = document.createElement("canvas");
    canvas.width = 256;
    canvas.height = 256;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas unavailable");
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(
      img,
      (img.width - size) / 2,
      (img.height - size) / 2,
      size,
      size,
      0,
      0,
      256,
      256,
    );
    preview.value = canvas.toDataURL("image/png");
  } catch (e) {
    showToast(String(e), "error");
  }
}

function readFile(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(new Error("读取图片失败"));
    reader.readAsDataURL(file);
  });
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("图片解析失败"));
    img.src = src;
  });
}

async function save() {
  if (!preview.value || saving.value) return;
  saving.value = true;
  try {
    await homeLogoSet(props.versionName, preview.value);
    showToast(t("home.logo.changed"), "success");
    emit("saved");
    emit("update:open", false);
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    saving.value = false;
  }
}

async function remove() {
  if (saving.value) return;
  saving.value = true;
  try {
    await homeLogoRemove(props.versionName);
    showToast(t("home.logo.removed"), "success");
    emit("saved");
    emit("update:open", false);
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="open"
      class="icon-dialog"
      role="dialog"
      aria-modal="true"
      @click.self="close"
    >
      <div class="icon-dialog__card">
        <header class="icon-dialog__header">
          <h2 class="icon-dialog__title">{{ t("home.logo.set") }}</h2>
          <button class="icon-dialog__close" :title="t('common.close')" @click="close">
            <X :size="16" />
          </button>
        </header>

        <div class="icon-dialog__body">
          <div class="icon-dialog__preview">
            <img
              v-if="preview ?? currentLogo"
              :src="preview ?? currentLogo ?? ''"
              alt=""
            />
            <ImageIcon v-else :size="40" :stroke-width="1.4" />
          </div>

          <p class="icon-dialog__hint">{{ t("home.logo.crop_hint") }}</p>

          <div class="icon-dialog__actions">
            <CoButton variant="secondary" @click="pickFile">
              <Upload :size="15" />
              <span>{{ t("home.logo.upload") }}</span>
            </CoButton>
            <CoButton
              v-if="currentLogo || preview"
              variant="ghost"
              :disabled="saving"
              @click="remove"
            >
              <Trash2 :size="15" />
              <span>{{ t("home.logo.remove") }}</span>
            </CoButton>
          </div>
        </div>

        <footer class="icon-dialog__footer">
          <CoButton variant="ghost" @click="close">{{ t("common.cancel") }}</CoButton>
          <CoButton
            variant="primary"
            :disabled="!preview || saving"
            @click="save"
          >
            {{ t("home.logo.save") }}
          </CoButton>
        </footer>

        <input
          ref="fileInput"
          type="file"
          accept="image/*"
          class="icon-dialog__file"
          @change="onFileChange"
        />
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.icon-dialog {
  position: fixed;
  inset: 0;
  z-index: 100;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.5);
  animation: copper-fade var(--copper-duration) var(--copper-easing);
}

.icon-dialog__card {
  width: min(360px, calc(100vw - 48px));
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.35);
  animation: copper-pop var(--copper-duration) var(--copper-easing);
  overflow: hidden;
}

.icon-dialog__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--copper-space-4) var(--copper-space-4) 0;
}

.icon-dialog__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
}

.icon-dialog__close {
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

.icon-dialog__close:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.icon-dialog__body {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-4);
}

.icon-dialog__preview {
  width: 128px;
  height: 128px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface-2);
  color: var(--copper-text-disabled);
  overflow: hidden;
}

.icon-dialog__preview img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.icon-dialog__hint {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
}

.icon-dialog__actions {
  display: flex;
  gap: var(--copper-space-2);
}

.icon-dialog__footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4);
  background: var(--copper-surface-2);
}

.icon-dialog__file {
  display: none;
}

@keyframes copper-fade {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

@keyframes copper-pop {
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
