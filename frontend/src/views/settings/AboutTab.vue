<script setup lang="ts">
// 关于 Tab：版本号、更新检查 / 安装、许可证、开源信息。

import { computed, onMounted, onUnmounted, ref } from "vue";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoButton from "../../components/ui/CoButton.vue";
import { kernelInfo } from "../../api/theme";
import {
  updaterApply,
  updaterCheck,
  updaterInstall,
  updaterStatus,
  type UpdateStatus,
} from "../../api/updater";
import { downloadTask, formatBytes } from "../../api/download";
import { onUpdateStatus, type Unlisten } from "../../events";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";

const { t } = useI18n();

const version = ref("0.1.0");
const status = ref<UpdateStatus | null>(null);
const downloadProgress = ref<{ downloaded: number; total: number } | null>(null);

let unlistenStatus: Unlisten | null = null;
let progressTimer: ReturnType<typeof setInterval> | null = null;

const checking = computed(() => status.value?.phase === "checking");
const downloading = computed(() => status.value?.phase === "downloading");
const downloaded = computed(() => status.value?.phase === "downloaded");

async function refreshVersion() {
  try {
    const info = await kernelInfo();
    version.value = info.version;
  } catch {
    // 内核未就绪时保持默认版本号。
  }
}

async function check() {
  try {
    status.value = await updaterCheck();
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function apply() {
  try {
    await updaterApply();
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function install() {
  try {
    await updaterInstall();
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function pollDownloadProgress() {
  const id = status.value?.download_task_id;
  if (id === null || id === undefined) return;
  try {
    const task = await downloadTask(id);
    if (task) {
      downloadProgress.value = {
        downloaded: task.downloaded_bytes,
        total: task.total_bytes,
      };
    }
  } catch {
    // 任务不存在则忽略。
  }
}

onMounted(async () => {
  await refreshVersion();
  try {
    status.value = await updaterStatus();
  } catch {
    // 内核未就绪。
  }
  unlistenStatus = await onUpdateStatus((next) => {
    status.value = next;
    if (next.phase === "downloading") {
      progressTimer = setInterval(() => void pollDownloadProgress(), 300);
    } else if (progressTimer) {
      clearInterval(progressTimer);
      progressTimer = null;
    }
  });
  if (status.value?.phase === "downloading") {
    progressTimer = setInterval(() => void pollDownloadProgress(), 300);
  }
});

onUnmounted(() => {
  unlistenStatus?.();
  if (progressTimer) clearInterval(progressTimer);
});
</script>

<template>
  <div class="about-tab">
    <SettingSection title-key="settings.about.app_info">
      <SettingRow label-key="settings.about.version">
        <span class="about-tab__version">{{ version }}</span>
      </SettingRow>

      <SettingRow label-key="settings.about.check_update">
        <template #default>
          <div class="about-tab__update">
            <template v-if="status?.phase === 'failed'">
              <span class="about-tab__error">{{ status.error }}</span>
              <CoButton size="sm" @click="check">{{ t("common.retry") }}</CoButton>
            </template>
            <template v-else-if="checking">
              <span class="about-tab__hint">{{ t("settings.about.checking") }}…</span>
            </template>
            <template v-else-if="downloading">
              <span class="about-tab__hint">
                {{ t("settings.about.updating") }}…
                <template v-if="downloadProgress?.total">
                  {{ formatBytes(downloadProgress.downloaded) }} /
                  {{ formatBytes(downloadProgress.total) }}
                </template>
              </span>
            </template>
            <template v-else-if="downloaded">
              <CoButton size="sm" variant="primary" @click="install">
                {{ t("settings.about.update_install") }}
              </CoButton>
            </template>
            <template v-else-if="status?.phase === 'available' && status.latest">
              <span class="about-tab__hint">
                {{ t("settings.about.update_available") }}：
                <strong>v{{ status.latest.version }}</strong>
              </span>
              <CoButton size="sm" variant="primary" @click="apply">
                {{ t("settings.about.update_install") }}
              </CoButton>
            </template>
            <template v-else>
              <span v-if="status" class="about-tab__hint">
                {{ t("settings.about.up_to_date") }}
              </span>
              <CoButton size="sm" @click="check">
                {{ t("settings.about.check_update") }}
              </CoButton>
            </template>
          </div>
        </template>
      </SettingRow>
    </SettingSection>

    <SettingSection title-key="settings.about.license">
      <SettingRow label-key="settings.about.open_source">
        <span class="about-tab__hint">{{ t("settings.about.license_text") }}</span>
      </SettingRow>
    </SettingSection>
  </div>
</template>

<style scoped>
.about-tab {
  max-width: 640px;
}

.about-tab__version {
  font-weight: 600;
}

.about-tab__update {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
}

.about-tab__hint {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.about-tab__error {
  color: var(--copper-danger);
  font-size: var(--copper-font-size-sm);
}
</style>
