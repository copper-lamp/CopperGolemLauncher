<script setup lang="ts">
// 通用 Tab：语言、外观深浅色、强调色（预设 / 自定义）、LLM 配置。

import { computed, onMounted, ref } from "vue";
import { Eye, EyeOff } from "@lucide/vue";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { llmAddModel, llmClearApiKey, llmListModels, llmRemoveModel, llmSetApiKey, llmUpdateModel, type LlmModelRow } from "../../api/llm";

const { t, locale, setLocale, supportedLocales } = useI18n();




const localeOptions = supportedLocales().map((code) => ({ value: code, label: code }));





async function changeLocale(code: string) {
  if (code === locale.value) return;
  await setLocale(code);
  showToast(t("common.success"), "success");
}



// ------------------------------------------------ LLM 模型表（内核唯一的 AI 接入读取点）
// 每条含 显示名称（可留空）/ 接口地址 / 模型名称；密钥经密钥环存储且永不回显，
// 仅展示「是否已配置」。内核不标记「默认模型」——调用方按 id 取用。
const llmRows = ref<LlmModelRow[]>([]);
const llmEditorOpen = ref(false);
const llmEditingId = ref<string | null>(null);
const llmFormDisplayName = ref("");
const llmFormBaseUrl = ref("");
const llmFormModel = ref("");
const llmFormApiKey = ref("");
const llmFormApiKeyConfigured = ref(false);
const llmShowFormKey = ref(false);
const llmSaving = ref(false);
const llmError = ref("");

// 有效显示名：显示名 trim 后非空则用它，否则取模型名——与内核规则保持一致。
const llmFormEffectiveName = computed(
  () => llmFormDisplayName.value.trim() || llmFormModel.value.trim(),
);

/** 取内核返回的 message（非 Error 时退回字符串化）。 */
function llmErrorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

async function loadLlmModels() {
  try {
    llmRows.value = await llmListModels();
  } catch (e) {
    showToast(llmErrorText(e), "error");
  }
}

function openAddLlm() {
  llmEditingId.value = null;
  llmFormDisplayName.value = "";
  llmFormBaseUrl.value = "";
  llmFormModel.value = "";
  llmFormApiKey.value = "";
  llmFormApiKeyConfigured.value = false;
  llmShowFormKey.value = false;
  llmError.value = "";
  llmEditorOpen.value = true;
}

function openEditLlm(row: LlmModelRow) {
  llmEditingId.value = row.id;
  llmFormDisplayName.value = row.display_name;
  llmFormBaseUrl.value = row.base_url;
  llmFormModel.value = row.model;
  // 密钥不回填：留空即「不修改」。
  llmFormApiKey.value = "";
  llmFormApiKeyConfigured.value = row.api_key_configured;
  llmShowFormKey.value = false;
  llmError.value = "";
  llmEditorOpen.value = true;
}

function cancelLlmEdit() {
  llmEditorOpen.value = false;
  llmError.value = "";
}

/** 提交前的基础校验：必填 + 明显重名（内核仍会再校验一次）。 */
function validateLlmForm(): string {
  if (!llmFormBaseUrl.value.trim()) return t("settings.general.llm_required_base_url");
  if (!llmFormModel.value.trim()) return t("settings.general.llm_required_model");
  const name = llmFormEffectiveName.value;
  const clash = llmRows.value.some(
    (row) => row.id !== llmEditingId.value && row.effective_name === name,
  );
  return clash ? t("settings.general.llm_name_conflict", { name }) : "";
}

async function saveLlmModel() {
  const invalid = validateLlmForm();
  if (invalid) {
    llmError.value = invalid;
    return;
  }
  llmSaving.value = true;
  llmError.value = "";
  const key = llmFormApiKey.value.trim();
  try {
    if (llmEditingId.value) {
      await llmUpdateModel(
        llmEditingId.value,
        llmFormDisplayName.value,
        llmFormBaseUrl.value,
        llmFormModel.value,
      );
      // 编辑时密钥留空 = 不修改，填了才写。
      if (key) await llmSetApiKey(llmEditingId.value, key);
    } else {
      const id = await llmAddModel(
        llmFormDisplayName.value,
        llmFormBaseUrl.value,
        llmFormModel.value,
      );
      if (key) await llmSetApiKey(id, key);
    }
    await loadLlmModels();
    llmEditorOpen.value = false;
    showToast(t("settings.general.llm_saved"), "success");
  } catch (e) {
    // 内核错误原样展示，不吞掉、不自造文案。
    llmError.value = llmErrorText(e);
  } finally {
    llmSaving.value = false;
  }
}

async function clearLlmFormKey() {
  if (!llmEditingId.value || !llmFormApiKeyConfigured.value) return;
  try {
    await llmClearApiKey(llmEditingId.value);
    await loadLlmModels();
    llmFormApiKeyConfigured.value = false;
    llmFormApiKey.value = "";
    showToast(t("settings.general.llm_key_cleared"), "success");
  } catch (e) {
    llmError.value = llmErrorText(e);
  }
}

async function removeLlmModel(row: LlmModelRow) {
  if (!window.confirm(t("settings.general.llm_delete_confirm", { name: row.effective_name }))) {
    return;
  }
  try {
    await llmRemoveModel(row.id);
    if (llmEditingId.value === row.id) llmEditorOpen.value = false;
    await loadLlmModels();
    showToast(t("settings.general.llm_deleted"), "success");
  } catch (e) {
    showToast(llmErrorText(e), "error");
  }
}

onMounted(loadLlmModels);
</script>

<template>
  <div class="general-tab">
    <SettingSection title-key="settings.tabs.general">
      <SettingRow label-key="settings.general.language" hint-key="settings.general.language_hint">
        <CoSelect
          :model-value="locale"
          :options="localeOptions"
          @update:model-value="changeLocale"
        />
      </SettingRow>

    </SettingSection>



    <SettingSection title-key="settings.general.llm">
      <div class="general-tab__llm">
        <div class="general-tab__llm-header">
          <p class="general-tab__llm-hint">{{ t("settings.general.llm_hint") }}</p>
          <CoButton variant="primary" size="sm" @click="openAddLlm">
            {{ t("settings.general.llm_add") }}
          </CoButton>
        </div>

        <p v-if="llmRows.length === 0" class="general-tab__llm-empty">
          {{ t("settings.general.llm_empty") }}
        </p>
        <div v-else class="general-tab__llm-table-wrap">
          <table class="general-tab__llm-table">
            <thead>
              <tr>
                <th>{{ t("settings.general.llm_col_name") }}</th>
                <th>{{ t("settings.general.llm_col_base_url") }}</th>
                <th>{{ t("settings.general.llm_col_model") }}</th>
                <th>{{ t("settings.general.llm_col_key") }}</th>
                <th class="general-tab__llm-col-actions">
                  {{ t("settings.general.llm_col_actions") }}
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in llmRows" :key="row.id">
                <td>
                  <span :class="{ 'general-tab__llm-name--derived': !row.display_name.trim() }">
                    {{ row.effective_name }}
                  </span>
                  <span v-if="!row.display_name.trim()" class="general-tab__llm-derived">
                    {{ t("settings.general.llm_name_from_model") }}
                  </span>
                </td>
                <td class="general-tab__llm-cell-url">{{ row.base_url }}</td>
                <td>{{ row.model }}</td>
                <td>
                  <span
                    :class="[
                      'general-tab__llm-key-status',
                      { 'general-tab__llm-key-status--on': row.api_key_configured },
                    ]"
                  >
                    {{
                      row.api_key_configured
                        ? t("settings.general.llm_configured")
                        : t("settings.general.llm_not_configured")
                    }}
                  </span>
                </td>
                <td class="general-tab__llm-col-actions">
                  <CoButton variant="ghost" size="sm" @click="openEditLlm(row)">
                    {{ t("settings.general.llm_edit") }}
                  </CoButton>
                  <CoButton variant="danger" size="sm" @click="removeLlmModel(row)">
                    {{ t("settings.general.llm_delete") }}
                  </CoButton>
                </td>
              </tr>
            </tbody>
          </table>
        </div>

        <div v-if="llmEditorOpen" class="general-tab__llm-editor">
          <div class="general-tab__llm-editor-title">
            {{
              llmEditingId
                ? t("settings.general.llm_edit_title")
                : t("settings.general.llm_add_title")
            }}
          </div>
          <div class="general-tab__llm-field">
            <span class="general-tab__llm-label">{{ t("settings.general.llm_display_name") }}</span>
            <CoTextField
              :model-value="llmFormDisplayName"
              :placeholder="t('settings.general.llm_display_name_placeholder')"
              @update:model-value="llmFormDisplayName = $event"
            />
            <span class="general-tab__llm-field-hint">
              {{ t("settings.general.llm_display_name_hint") }}
            </span>
          </div>
          <div class="general-tab__llm-field">
            <span class="general-tab__llm-label">{{ t("settings.general.llm_base_url") }}</span>
            <CoTextField
              :model-value="llmFormBaseUrl"
              :placeholder="t('settings.general.llm_base_url_placeholder')"
              @update:model-value="llmFormBaseUrl = $event"
            />
          </div>
          <div class="general-tab__llm-field">
            <span class="general-tab__llm-label">{{ t("settings.general.llm_model") }}</span>
            <CoTextField
              :model-value="llmFormModel"
              :placeholder="t('settings.general.llm_model_placeholder')"
              @update:model-value="llmFormModel = $event"
            />
          </div>
          <div class="general-tab__llm-field">
            <span class="general-tab__llm-label">{{ t("settings.general.llm_api_key") }}</span>
            <div class="general-tab__llm-key">
              <CoTextField
                :model-value="llmFormApiKey"
                :type="llmShowFormKey ? 'text' : 'password'"
                :placeholder="
                  llmEditingId
                    ? t('settings.general.llm_api_key_keep_hint')
                    : t('settings.general.llm_api_key_optional_hint')
                "
                @update:model-value="llmFormApiKey = $event"
                @enter="saveLlmModel"
              />
              <CoButton
                variant="ghost"
                size="sm"
                :title="t('settings.general.llm_api_key')"
                @click="llmShowFormKey = !llmShowFormKey"
              >
                <EyeOff v-if="llmShowFormKey" :size="16" />
                <Eye v-else :size="16" />
              </CoButton>
            </div>
            <span class="general-tab__llm-field-hint">
              {{ t("settings.general.llm_api_key_hint") }}
            </span>
          </div>

          <div class="general-tab__llm-editor-actions">
            <CoButton
              variant="danger"
              size="sm"
              :disabled="!llmEditingId || !llmFormApiKeyConfigured"
              @click="clearLlmFormKey"
            >
              {{ t("settings.general.llm_clear_key") }}
            </CoButton>
            <span class="general-tab__llm-spacer" />
            <CoButton variant="secondary" size="sm" :disabled="llmSaving" @click="cancelLlmEdit">
              {{ t("settings.general.llm_cancel") }}
            </CoButton>
            <CoButton variant="primary" size="sm" :disabled="llmSaving" @click="saveLlmModel">
              {{ t("settings.general.llm_save") }}
            </CoButton>
          </div>
          <p v-if="llmError" class="general-tab__llm-error">{{ llmError }}</p>
        </div>
      </div>
    </SettingSection>
  </div>
</template>

<style scoped>
.general-tab {
  max-width: 640px;
}

.general-tab__accent {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.general-tab__swatch {
  width: 26px;
  height: 26px;
  border: 2px solid transparent;
  border-radius: var(--copper-radius-full);
  cursor: pointer;
  transition:
    transform var(--copper-duration-fast) var(--copper-easing),
    border-color var(--copper-duration-fast) var(--copper-easing);
}

.general-tab__swatch:hover {
  transform: scale(1.12);
}

.general-tab__swatch--active {
  border-color: var(--copper-text);
  transform: scale(1.08);
}

.general-tab__accent-input {
  width: 96px;
}

.general-tab__llm {
  padding: var(--copper-space-3) 0;
}

.general-tab__llm-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--copper-space-3);
  margin-bottom: var(--copper-space-3);
}

.general-tab__llm-hint {
  margin: 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  line-height: 1.5;
}

.general-tab__llm-empty {
  margin: 0;
  padding: var(--copper-space-4) 0;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  text-align: center;
}

.general-tab__llm-table-wrap {
  overflow-x: auto;
}

.general-tab__llm-table {
  width: 100%;
  min-width: 600px;
  border-collapse: collapse;
  font-size: var(--copper-font-size-sm);
}

.general-tab__llm-table th {
  padding: var(--copper-space-2);
  border-bottom: 1px solid var(--copper-border);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  font-weight: 600;
  text-align: left;
  white-space: nowrap;
}

.general-tab__llm-table td {
  padding: var(--copper-space-2);
  border-bottom: 1px solid var(--copper-border);
  vertical-align: middle;
}

.general-tab__llm-table tbody tr:last-child td {
  border-bottom: none;
}

.general-tab__llm-name--derived {
  color: var(--copper-text-secondary);
  font-style: italic;
}

.general-tab__llm-derived {
  display: block;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.general-tab__llm-cell-url {
  max-width: 200px;
  overflow-wrap: anywhere;
  color: var(--copper-text-secondary);
}

.general-tab__llm-key-status {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
}

.general-tab__llm-key-status--on {
  color: var(--copper-accent);
}

.general-tab__llm-col-actions {
  text-align: right;
  white-space: nowrap;
}

.general-tab__llm-editor {
  margin-top: var(--copper-space-3);
  padding: var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
}

.general-tab__llm-editor-title {
  margin-bottom: var(--copper-space-3);
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

.general-tab__llm-field {
  margin-bottom: var(--copper-space-3);
}

.general-tab__llm-label {
  display: block;
  margin-bottom: var(--copper-space-1);
  font-size: var(--copper-font-size-sm);
}

.general-tab__llm-field-hint {
  display: block;
  margin-top: var(--copper-space-1);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.general-tab__llm-key {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.general-tab__llm-editor-actions {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  margin-top: var(--copper-space-3);
}

.general-tab__llm-spacer {
  flex: 1;
}

.general-tab__llm-error {
  margin: var(--copper-space-2) 0 0;
  color: var(--copper-danger);
  font-size: var(--copper-font-size-xs);
}
</style>
