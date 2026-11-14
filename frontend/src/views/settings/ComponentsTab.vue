<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { LoaderCircle } from '@lucide/vue';
import SettingSection from './SettingSection.vue';
import SettingRow from './SettingRow.vue';
import { useI18n } from '../../i18n';
import { contentDownloadLipEnv, type LipEnv } from '../../modules/content-download/api';

const { t } = useI18n();
const lip = ref<LipEnv | null>(null);
const loading = ref(false);

async function refresh(): Promise<void> {
  loading.value = true;
  try {
    lip.value = await contentDownloadLipEnv();
  } finally {
    loading.value = false;
  }
}

onMounted(() => { void refresh(); });
</script>

<template>
  <SettingSection title-key="components.title">
    <SettingRow label-key="components.lipStatus">
      <LoaderCircle v-if="loading" :size="16" class="components-spin" />
      <span v-else>{{ lip?.lipAvailable ? t('components.builtinReady') : (lip?.message ?? t('components.unavailable')) }}</span>
    </SettingRow>
  </SettingSection>
</template>

<style scoped>
.components-spin { flex-shrink: 0; animation: components-spin 1s linear infinite; }
html[data-motion="reduced"] .components-spin { animation: none; }
@keyframes components-spin { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .components-spin { animation: none; } }
</style>
