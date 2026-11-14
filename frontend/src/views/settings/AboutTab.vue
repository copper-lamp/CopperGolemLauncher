<script setup lang="ts">
// 关于 Tab：版本、赞助入口、许可证、开发预览。
// 赞助信息是组织公开渠道，失败时只提示不外抛，不影响其余关于信息。

import { ref } from "vue";
import { Heart } from "@lucide/vue";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";

import OnboardingHost from "../../components/onboarding/OnboardingHost.vue";
import { onboardingKey } from "../../services/onboardingMessages";
import SettingSection from "./SettingSection.vue";
import SettingRow from "./SettingRow.vue";
import CoButton from "../../components/ui/CoButton.vue";
import { useUpdate } from "../../composables/useUpdate";
import { showToast } from "../../composables/useToast";
import { clearPreviewUpdateState, nextPreviewLabelKey, previewNextUpdateState } from "../../updatePreview";
import { useI18n } from "../../i18n";

const { t } = useI18n();
const { status } = useUpdate();
const isDev = import.meta.env.DEV;
const onboardingPreview = ref(false);

const SPONSOR_URL = "https://afdian.com/a/CopperLamp";

async function openSponsor(): Promise<void> {
  try {
    await openExternal(SPONSOR_URL);
  } catch {
    showToast(t("settings.about.sponsor_failed"), "error");
  }
}
</script>

<template>
  <div class="about-tab">
    <SettingSection title-key="settings.about.app_info">
      <SettingRow label-key="settings.about.version">
        <span>{{ status?.current_version ?? "—" }}</span>
      </SettingRow>
    </SettingSection>

    <SettingSection title-key="settings.about.sponsor">
      <SettingRow label-key="settings.about.sponsor_channel" hint-key="settings.about.sponsor_hint">
        <CoButton variant="primary" size="sm" @click="openSponsor">
          <Heart :size="14" />
          <span>{{ t("settings.about.sponsor_action") }}</span>
        </CoButton>
      </SettingRow>
    </SettingSection>

    <SettingSection title-key="settings.about.license">
      <SettingRow label-key="settings.about.open_source">
        <span>{{ t("settings.about.license_text") }}</span>
      </SettingRow>
    </SettingSection>

    <SettingSection v-if="isDev" title-key="settings.about.dev_preview">
      <SettingRow label-key="settings.about.dev_preview" hint-key="settings.about.dev_preview_hint">
        <CoButton @click="previewNextUpdateState">{{ t(nextPreviewLabelKey()) }}</CoButton>
        <CoButton variant="ghost" @click="clearPreviewUpdateState">{{ t("common.cancel") }}</CoButton>
      </SettingRow>
    </SettingSection>

    <SettingSection v-if="isDev" :title-key="onboardingKey('preview.action')">
      <SettingRow :label-key="onboardingKey('preview.action')" :hint-key="onboardingKey('preview.notice')">
        <CoButton @click="onboardingPreview = true">{{ t(onboardingKey("preview.action")) }}</CoButton>
      </SettingRow>
    </SettingSection>

    <OnboardingHost v-if="isDev && onboardingPreview" preview @closed="onboardingPreview = false" />
  </div>
</template>

<style scoped>
.about-tab {
  max-width: 760px;
}
</style>
