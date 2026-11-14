<script setup lang="ts">
// 储存位置 Tab：游戏数据目录 + 启动器数据目录快捷入口。
//
// 这里只负责定位，不承担迁移：改目录就是改 game.directory，生效路径由内核
// 重新解析后回显（与「设置 → 版本」同一条链路，两个入口改的是同一个设置项）。
// 启动器数据目录是安装目录下的运行时数据（数据库 / 日志 / 缓存），由内核决定，
// 因此只读展示，提供系统文件管理器入口。

import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { FolderOpen, RotateCcw } from "@lucide/vue";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import { useSettings } from "../../composables/useSettings";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { homeVersionsRoot } from "../../api/home";
import { storageStatus } from "../../api/storage";

const { t } = useI18n();
const { get, set } = useSettings();

const resolvedRoot = ref("");
const launcherRoot = ref("");
const environmentOverride = ref(false);

const gameDir = computed({
  get: () => get<string>("game.directory", ""),
  set: (value: string) => void set("game.directory", value),
});

// 设置值与内核实际解析路径不一致 = 有进程级环境变量在覆盖，必须说破，
// 否则用户会反复修改一个不生效的设置项。
const overridden = computed(
  () => gameDir.value.trim() !== "" && resolvedRoot.value !== gameDir.value.trim(),
);

async function refresh() {
  try {
    const [root, status] = await Promise.all([homeVersionsRoot(), storageStatus()]);
    resolvedRoot.value = root;
    launcherRoot.value = status.launcherRoot;
    environmentOverride.value = status.environmentOverride;
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function pickFolder() {
  const selected = await open({ directory: true, multiple: false });
  if (typeof selected === "string" && selected) gameDir.value = selected;
}

function resetDir() {
  gameDir.value = "";
  void refresh();
}

async function reveal(path: string) {
  if (!path) return;
  try {
    await openPath(path);
  } catch (e) {
    showToast(String(e), "error");
  }
}

// 逐字符输入时不需要每次扫盘；去抖后只刷新一次生效路径。
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
watch(gameDir, () => {
  if (refreshTimer !== undefined) clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => void refresh(), 300);
});
onBeforeUnmount(() => {
  if (refreshTimer !== undefined) clearTimeout(refreshTimer);
});

onMounted(refresh);
</script>

<template>
  <div class="storage-tab">
    <SettingSection title-key="settings.storage.section">
      <SettingRow
        label-key="settings.storage.game_dir"
        hint-key="settings.storage.game_dir_hint"
      >
        <div class="storage-tab__row">
          <CoTextField
            class="storage-tab__field"
            :model-value="gameDir"
            :placeholder="t('settings.storage.game_dir_default')"
            :disabled="environmentOverride"
            @update:model-value="gameDir = $event"
          />
          <CoButton
            variant="secondary"
            size="sm"
            :disabled="environmentOverride"
            @click="pickFolder"
          >
            <FolderOpen :size="14" />
            <span>{{ t("settings.storage.browse") }}</span>
          </CoButton>
          <CoButton variant="ghost" size="sm" :disabled="environmentOverride" @click="resetDir">
            <RotateCcw :size="14" />
            <span>{{ t("settings.storage.reset") }}</span>
          </CoButton>
          <CoButton variant="secondary" size="sm" @click="reveal(resolvedRoot)">
            <FolderOpen :size="14" />
            <span>{{ t("settings.storage.open") }}</span>
          </CoButton>
        </div>
      </SettingRow>
      <p class="storage-tab__resolved">
        {{ t("settings.storage.resolved", { path: resolvedRoot || "—" }) }}
      </p>
      <p v-if="overridden || environmentOverride" class="storage-tab__warn">
        {{ t("settings.storage.overridden") }}
      </p>

      <SettingRow
        label-key="settings.storage.launcher_dir"
        hint-key="settings.storage.launcher_dir_hint"
      >
        <div class="storage-tab__row">
          <span class="storage-tab__path">{{ launcherRoot || "—" }}</span>
          <CoButton variant="secondary" size="sm" @click="reveal(launcherRoot)">
            <FolderOpen :size="14" />
            <span>{{ t("settings.storage.open") }}</span>
          </CoButton>
        </div>
      </SettingRow>
    </SettingSection>
  </div>
</template>

<style scoped>
.storage-tab {
  max-width: 760px;
}

.storage-tab__row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--copper-space-2);
}

.storage-tab__field {
  flex: 1 1 260px;
  min-width: 200px;
}

.storage-tab__path {
  flex: 1 1 260px;
  min-width: 0;
  overflow-wrap: anywhere;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.storage-tab__resolved {
  margin: 0 var(--copper-space-2) var(--copper-space-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.storage-tab__warn {
  margin: 0 var(--copper-space-2) var(--copper-space-3);
  color: var(--copper-warning, var(--copper-danger));
  font-size: var(--copper-font-size-xs);
}
</style>
