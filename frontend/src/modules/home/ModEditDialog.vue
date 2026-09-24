<script setup lang="ts">
// 模组清单编辑弹窗：改写 `name / entry / version / type / author`。
// `type` 为自由文本、不设白名单（与 LeviLauncher 一致），仅做非空校验；
// 清单里的未知字段由后端原样保留（见 `mods::save_manifest_at`）。

import { computed, ref, watch } from "vue";
import { X } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoButton from "../../components/ui/CoButton.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { homeModsSaveManifest, type ModView } from "../../api/home";

const props = defineProps<{
  open: boolean;
  versionName: string;
  mod: ModView | null;
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
  saved: [];
}>();

const { t } = useI18n();

const name = ref("");
const entry = ref("");
const version = ref("");
const modType = ref("");
const author = ref("");
const saving = ref(false);

// 每次打开都从当前模组重新取值。
watch(
  () => props.open,
  (open) => {
    if (!open || !props.mod) return;
    name.value = props.mod.name;
    entry.value = props.mod.entry;
    version.value = props.mod.version;
    modType.value = props.mod.mod_type;
    author.value = props.mod.author;
  },
);

const canSave = computed(
  () =>
    name.value.trim().length > 0 &&
    entry.value.trim().length > 0 &&
    version.value.trim().length > 0 &&
    modType.value.trim().length > 0,
);

function close() {
  if (saving.value) return;
  emit("update:open", false);
}

async function save() {
  const target = props.mod;
  if (!target || !canSave.value || saving.value) return;
  saving.value = true;
  try {
    await homeModsSaveManifest(
      props.versionName,
      target.folder,
      name.value.trim(),
      entry.value.trim(),
      version.value.trim(),
      modType.value.trim(),
      author.value.trim(),
    );
    showToast(t("module.home.mods.saved"), "success");
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
    <div v-if="open && mod" class="mod-dialog" role="dialog" aria-modal="true" @click.self="close">
      <div class="mod-dialog__card">
        <header class="mod-dialog__header">
          <h2 class="mod-dialog__title">{{ t("module.home.mods.edit_title") }}</h2>
          <button class="mod-dialog__close" :title="t('common.close')" @click="close">
            <X :size="16" />
          </button>
        </header>

        <div class="mod-dialog__body">
          <label class="mod-dialog__field">
            <span class="mod-dialog__label">{{ t("module.home.mods.field_name") }}</span>
            <CoTextField v-model="name" />
          </label>

          <label class="mod-dialog__field">
            <span class="mod-dialog__label">{{ t("module.home.mods.field_entry") }}</span>
            <CoTextField v-model="entry" />
            <span class="mod-dialog__hint">{{ t("module.home.mods.entry_hint") }}</span>
          </label>

          <div class="mod-dialog__pair">
            <label class="mod-dialog__field">
              <span class="mod-dialog__label">{{ t("module.home.mods.field_version") }}</span>
              <CoTextField v-model="version" />
            </label>
            <label class="mod-dialog__field">
              <span class="mod-dialog__label">{{ t("module.home.mods.field_type") }}</span>
              <CoTextField v-model="modType" />
            </label>
          </div>
          <span class="mod-dialog__hint">{{ t("module.home.mods.type_hint") }}</span>

          <label class="mod-dialog__field">
            <span class="mod-dialog__label">{{ t("module.home.mods.field_author") }}</span>
            <CoTextField v-model="author" />
          </label>
        </div>

        <footer class="mod-dialog__footer">
          <CoButton variant="ghost" @click="close">{{ t("common.cancel") }}</CoButton>
          <CoButton variant="primary" :disabled="!canSave || saving" @click="save">
            {{ t("module.home.mods.save") }}
          </CoButton>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.mod-dialog {
  position: fixed;
  inset: 0;
  z-index: 100;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.5);
  animation: mod-dialog-fade var(--copper-duration) var(--copper-easing);
}

.mod-dialog__card {
  width: min(420px, calc(100vw - 48px));
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.35);
  animation: mod-dialog-pop var(--copper-duration) var(--copper-easing);
  overflow: hidden;
}

.mod-dialog__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--copper-space-4) var(--copper-space-4) 0;
}

.mod-dialog__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
}

.mod-dialog__close {
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

.mod-dialog__close:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.mod-dialog__body {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  padding: var(--copper-space-4);
}

.mod-dialog__field {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
  min-width: 0;
}

.mod-dialog__pair {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--copper-space-3);
}

.mod-dialog__label {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.mod-dialog__hint {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.mod-dialog__footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--copper-space-2);
  padding: var(--copper-space-3) var(--copper-space-4);
  background: var(--copper-surface-2);
}

@keyframes mod-dialog-fade {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

@keyframes mod-dialog-pop {
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