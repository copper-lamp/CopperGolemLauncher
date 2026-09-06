<script setup lang="ts">
// 模块 Tab：模块列表（启用 / 停用）。
//
// 内核阶段无内置模块，列表通常为空；模块开发阶段由各模块注册后展示。

import { onMounted, ref } from "vue";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoSwitch from "../../components/ui/CoSwitch.vue";
import { modulesList, modulesSetEnabled, type ModuleInfo } from "../../api/modules";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";

const { t } = useI18n();

const modules = ref<ModuleInfo[]>([]);
const loading = ref(false);

async function refresh() {
  loading.value = true;
  try {
    modules.value = await modulesList();
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    loading.value = false;
  }
}

async function toggleEnabled(module: ModuleInfo, enabled: boolean) {
  try {
    await modulesSetEnabled(module.id, enabled);
    module.enabled = enabled;
  } catch (e) {
    showToast(String(e), "error");
  }
}

function stateLabel(module: ModuleInfo): string {
  if (!module.enabled) return t("settings.modules.disabled");
  return t("settings.modules.enabled");
}

onMounted(refresh);
</script>

<template>
  <div class="modules-tab">
    <SettingSection title-key="settings.modules.module_list">
      <div v-if="loading" class="modules-tab__empty">{{ t("common.loading") }}</div>
      <div v-else-if="modules.length === 0" class="modules-tab__empty">
        {{ t("settings.modules.no_modules") }}
      </div>
      <SettingRow
        v-for="module in modules"
        :key="module.id"
        :label-key="module.id"
        :hint-key="undefined"
      >
        <template #default>
          <div class="modules-tab__item">
            <span
              :class="['modules-tab__state', { 'modules-tab__state--on': module.enabled }]"
            >
              {{ stateLabel(module) }}
            </span>
            <CoSwitch :model-value="module.enabled" @update:model-value="toggleEnabled(module, $event)" />
          </div>
        </template>
      </SettingRow>
    </SettingSection>
  </div>
</template>

<style scoped>
.modules-tab {
  max-width: 640px;
}

.modules-tab__empty {
  padding: var(--copper-space-6) 0;
  text-align: center;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.modules-tab__item {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
}

.modules-tab__state {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.modules-tab__state--on {
  color: var(--copper-success);
}
</style>
