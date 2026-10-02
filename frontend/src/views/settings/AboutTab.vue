<script setup lang="ts">
// 关于 Tab：版本号、手动检查更新、更新偏好、许可证、开源信息。
//
// 与标题栏更新面板共用 `useUpdate` 单例状态，因此这里不需要自己订阅事件，
// 也不需要轮询下载任务：进度、阶段、错误都已经是现成的响应式数据。
import { computed, ref } from "vue";
import { LoaderCircle, RefreshCw } from "@lucide/vue";

import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoSwitch from "../../components/ui/CoSwitch.vue";
import CoProgress from "../../components/ui/CoProgress.vue";
import { useUpdate } from "../../composables/useUpdate";
import { useSettings } from "../../composables/useSettings";
import { formatBytes, formatSpeed } from "../../api/download";
import { formatEta } from "../../components/update/updateFormat";
import { useI18n } from "../../i18n";

const { t } = useI18n();
const { get, set } = useSettings();
const {
  status,
  phase,
  latest,
  progress,
  busy,
  error,
  errorKind,
  downloading,
  versionDelta,
  check,
  cancel,
  openPanel,
} = useUpdate();

/** 手动检查的失败原因（IPC 层异常，后端业务失败走 `error`）。 */
const checkError = ref<string | null>(null);

/** 启动后自动检查开关。 */
const autoCheck = computed({
  get: () => get<boolean>("update.auto_check", true),
  set: (value: boolean) => void set("update.auto_check", value),
});

const checking = computed(() => phase.value === "checking");
const downloaded = computed(() => phase.value === "downloaded");
const hasUpdate = computed(() => phase.value === "available" && latest.value != null);

/** 手动检查：成功与失败都在本页给出明确反馈。 */
async function onCheck() {
  checkError.value = null;
  try {
    await check();
  } catch (e) {
    checkError.value = e instanceof Error ? e.message : String(e);
  }
}

/** 失败文案（后端原文优先）。 */
const failureText = computed(() => error.value ?? checkError.value);

function failureHint(): string {
  if (errorKind.value === "no_asset") return t("update.failed_no_asset_hint");
  if (errorKind.value === "download") return t("update.failed_download_hint");
  return t("update.failed_network_hint");
}
</script>

<template>
  <div class="about-tab">
    <SettingSection title-key="settings.about.app_info">
      <SettingRow label-key="settings.about.version">
        <span class="about-tab__version">{{ status?.current_version ?? "—" }}</span>
      </SettingRow>

      <SettingRow
        label-key="settings.about.check_update"
        hint-key="settings.about.check_update_hint"
      >
        <div class="about-tab__check">
          <CoButton
            variant="secondary"
            size="sm"
            :disabled="checking || busy"
            @click="onCheck"
          >
            <LoaderCircle v-if="checking" class="about-tab__spin" :size="14" />
            <RefreshCw v-else :size="14" />
            <span>{{ t("update.check_now") }}</span>
          </CoButton>
          <!-- 有更新时主入口是「查看更新」，直接打开面板而不是在这重做一遍流程。 -->
          <CoButton v-if="hasUpdate || downloading || downloaded" size="sm" variant="primary" @click="openPanel">
            {{ downloading ? t("update.badge.downloading") : downloaded ? t("update.badge.ready") : t("update.badge.available") }}
          </CoButton>
        </div>
      </SettingRow>

      <!-- 状态区：与「检查」按钮同宽内纵向排布，避免横向挤爆设置行 -->
      <div v-if="status" class="about-tab__state">
        <!-- 检查中：不确定态进度，明说在等网络 -->
        <div v-if="checking" class="about-tab__state-block">
          <CoProgress :value="null" size="sm" />
          <span class="about-tab__state-text">{{ t("update.checking_hint") }}</span>
        </div>

        <!-- 下载中：进度条 + 字节 + 速度 + ETA -->
        <div v-else-if="downloading" class="about-tab__state-block">
          <CoProgress :value="progress?.ratio ?? null" size="sm" />
          <div class="about-tab__state-row">
            <span class="about-tab__state-text">
              <template v-if="progress?.total">
                {{ formatBytes(progress.downloaded) }} / {{ formatBytes(progress.total) }}
              </template>
              <template v-else>{{ formatBytes(progress?.downloaded ?? 0) }}</template>
            </span>
            <span v-if="progress?.speed" class="about-tab__state-dim">
              {{ formatSpeed(progress.speed) }}
              <template v-if="progress.etaSeconds != null">
                · {{ t("update.eta", { seconds: formatEta(progress.etaSeconds) }) }}
              </template>
            </span>
            <CoButton variant="ghost" size="sm" :disabled="busy" @click="cancel">
              {{ t("update.cancel_download") }}
            </CoButton>
          </div>
        </div>

        <!-- 已就绪 -->
        <div v-else-if="downloaded" class="about-tab__state-block">
          <CoProgress :value="1" size="sm" tone="success" />
          <span class="about-tab__state-text">{{ t("update.ready_body") }}</span>
        </div>

        <!-- 失败：原文 + 分类提示 + 重试 -->
        <div v-else-if="phase === 'failed'" class="about-tab__state-block">
          <span class="about-tab__state-error">{{ failureText ?? t("update.failed_generic") }}</span>
          <span class="about-tab__state-dim">{{ failureHint() }}</span>
        </div>

        <!-- 有更新（未开始下载） -->
        <div v-else-if="hasUpdate && latest" class="about-tab__state-block">
          <span class="about-tab__state-text">
            {{ t("update.available_title") }}
            <template v-if="versionDelta"> v{{ versionDelta.next }}</template>
            <template v-if="latest.asset_size"> · {{ formatBytes(latest.asset_size) }}</template>
          </span>
        </div>

        <!-- 已是最新 -->
        <div v-else-if="status.phase === 'idle'" class="about-tab__state-block">
          <span class="about-tab__state-text">
            {{ t("update.up_to_date_body", { version: status.current_version }) }}
          </span>
          <span v-if="failureText" class="about-tab__state-dim">{{ failureText }}</span>
        </div>
      </div>
    </SettingSection>

    <SettingSection title-key="settings.about.update_prefs">
      <SettingRow
        label-key="settings.about.auto_check"
        hint-key="settings.about.auto_check_hint"
      >
        <CoSwitch
          :model-value="autoCheck"
          @update:model-value="autoCheck = $event"
        />
      </SettingRow>
      <SettingRow label-key="settings.about.update_channel" :hint-key="settings.about.update_channel_hint">
        <span class="about-tab__version">{{ get<string>("update.channel", "stable") }}</span>
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

.about-tab__check {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.about-tab__state {
  padding: 0 0 var(--copper-space-3);
}

.about-tab__state-block {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.about-tab__state-row {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
}

.about-tab__state-text {
  color: var(--copper-text);
  font-size: var(--copper-font-size-sm);
}

.about-tab__state-dim {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
}

.about-tab__state-error {
  color: var(--copper-danger);
  font-size: var(--copper-font-size-sm);
  overflow-wrap: anywhere;
}

.about-tab__spin {
  animation: about-tab-rotate 1s linear infinite;
}

.about-tab__hint {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

@keyframes about-tab-rotate {
  to {
    transform: rotate(360deg);
  }
}
</style>