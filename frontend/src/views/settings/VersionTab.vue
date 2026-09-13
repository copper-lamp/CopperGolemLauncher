<script setup lang="ts">
// 版本设置 Tab：游戏目录（版本根）+ 下载目标版本。
// - 游戏目录：`game.directory`（空 = 默认 %APPDATA%/.../versions），可选择文件夹 / 恢复默认；
// - 下载目标版本：`launch.default_version`，内容下载落盘到该版本（空 = 仅下载不安装）。

import { computed, onMounted, ref } from "vue";
import { FolderOpen, RotateCcw } from "@lucide/vue";
import { open } from "@tauri-apps/plugin-dialog";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { homeVersionsList, homeVersionsRoot, type VersionView } from "../../api/home";

const { t } = useI18n();
const { get, set } = useSettings();

const versions = ref<VersionView[]>([]);
const resolvedRoot = ref("");

const gameDir = computed({
  get: () => get<string>("game.directory", ""),
  set: (value: string) => void set("game.directory", value),
});

const targetVersion = computed({
  get: () => get<string>("launch.default_version", ""),
  set: (value: string) => void set("launch.default_version", value),
});

const versionOptions = computed(() => [
  { value: "", label: t("settings.version.target_none") },
  ...versions.value.map((v) => ({ value: v.name, label: v.name })),
]);

async function refreshVersions() {
  try {
    versions.value = await homeVersionsList();
    resolvedRoot.value = await homeVersionsRoot();
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function pickFolder() {
  const selected = await open({ directory: true, multiple: false });
  if (typeof selected === "string" && selected) {
    gameDir.value = selected;
  }
}

function resetDir() {
  gameDir.value = "";
  void refreshVersions();
}

onMounted(refreshVersions);
</script>

<template>
  <div class="version-tab">
    <SettingSection :title-key="'settings.version.game_dir'">
      <SettingRow
        :label-key="'settings.version.game_dir_label'"
        :hint-key="'settings.version.game_dir_hint'"
      >
        <div class="version-tab__dir">
          <CoTextField
            :model-value="gameDir"
            :placeholder="t('settings.version.game_dir_default')"
            @update:model-value="gameDir = $event"
          />
          <CoButton variant="secondary" size="sm" @click="pickFolder">
            <FolderOpen :size="14" />
            <span>{{ t("settings.version.browse") }}</span>
          </CoButton>
          <CoButton variant="secondary" size="sm" @click="resetDir">
            <RotateCcw :size="14" />
            <span>{{ t("settings.version.reset") }}</span>
          </CoButton>
        </div>
      </SettingRow>
      <div class="version-tab__resolved">
        {{ t("settings.version.resolved", { path: resolvedRoot }) }}
      </div>
    </SettingSection>

    <SettingSection :title-key="'settings.version.content_target'">
      <SettingRow
        :label-key="'settings.version.target_label'"
        :hint-key="'settings.version.target_hint'"
      >
        <CoSelect
          :model-value="targetVersion"
          :options="versionOptions"
          @update:model-value="targetVersion = $event"
        />
      </SettingRow>
    </SettingSection>
  </div>
</template>

<style scoped>
.version-tab {
  max-width: 680px;
}

.version-tab__dir {
  display: flex;
  gap: var(--copper-space-2);
  align-items: center;
  min-width: 380px;
}

.version-tab__dir :deep(.co-text-field) {
  flex: 1;
}

.version-tab__resolved {
  padding: var(--copper-space-2) 0 var(--copper-space-3);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  overflow-wrap: anywhere;
}
</style>