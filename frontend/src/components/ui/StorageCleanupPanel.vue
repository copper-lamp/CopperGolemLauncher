<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ArrowLeft, Check, Trash2 } from "@lucide/vue";
import { useI18n } from "../../i18n";
import { formatBytes } from "../../api/download";
import type { StorageUsage } from "../../api/storage";
import CoButton from "./CoButton.vue";
import CoTextField from "./CoTextField.vue";
import SettingSection from "../../views/settings/SettingSection.vue";
import SettingRow from "../../views/settings/SettingRow.vue";

const props = defineProps<{ usage: StorageUsage | null; resolvedRoot: string; gameDir: string; environmentOverride: boolean; isAndroid: boolean }>();
const emit = defineEmits<{ back: []; clean: [categories: string[]]; "update:gameDir": [value: string]; browse: []; reset: []; open: [] }>();
const { t } = useI18n();
const selected = ref<string[]>([]);
const confirmOpen = ref(false);
const cleanable = computed(() => (props.usage?.categories ?? []).filter(item => item.cleanable));
const selectedBytes = computed(() => cleanable.value.filter(item => selected.value.includes(item.id)).reduce((sum, item) => sum + item.bytes, 0));
watch(cleanable, items => { selected.value = items.map(item => item.id); }, { immediate: true });
function toggle(id: string) { selected.value = selected.value.includes(id) ? selected.value.filter(item => item !== id) : [...selected.value, id]; }
function confirmClean() { confirmOpen.value = false; emit("clean", [...selected.value]); }
</script>

<template>
  <div class="cleanup">
    <header class="cleanup__header">
      <CoButton variant="ghost" size="sm" :aria-label="t('settings.storage.cleanup.back')" @click="emit('back')"><ArrowLeft :size="16"/><span>{{ t("settings.storage.cleanup.back") }}</span></CoButton>
      <div><h2>{{ t("settings.storage.cleanup.title") }}</h2><p>{{ t("settings.storage.cleanup.desc") }}</p></div>
    </header>
    <section v-if="usage" class="cleanup__list">
      <div class="cleanup__toolbar">
        <label><input type="checkbox" :checked="selected.length === cleanable.length && cleanable.length > 0" @change="selected = selected.length === cleanable.length ? [] : cleanable.map(item => item.id)"/>{{ t("settings.storage.cleanup.select_all") }}</label>
        <span>{{ t("settings.storage.cleanup.selected", { count: selected.length }) }}</span>
      </div>
      <label v-for="item in usage.categories" :key="item.id" class="cleanup__item" :class="{ 'cleanup__item--disabled': !item.cleanable }">
        <input type="checkbox" :checked="selected.includes(item.id)" :disabled="!item.cleanable" @change="toggle(item.id)"/>
        <span class="cleanup__name">{{ t(`settings.storage.usage.category.${item.id}`) }}<small v-if="!item.cleanable">{{ t("settings.storage.cleanup.not_cleanable_hint") }}</small></span>
        <strong>{{ formatBytes(item.bytes) }}</strong>
      </label>
      <div v-if="!usage.categories.length" class="cleanup__empty">{{ t("settings.storage.cleanup.empty") }}</div>
      <footer><span>{{ t("settings.storage.cleanup.free") }} <strong>{{ formatBytes(selectedBytes) }}</strong></span><CoButton variant="danger" :disabled="!selected.length" @click="confirmOpen = true"><Trash2 :size="16"/><span>{{ t("settings.storage.cleanup.confirm") }}</span></CoButton></footer>
    </section>
    <section v-else class="cleanup__list"><p class="cleanup__empty">{{ t("settings.storage.cleanup.empty") }}</p></section>

    <SettingSection v-if="!isAndroid" :title-key="'settings.storage.game_dir'">
      <SettingRow label-key="settings.storage.game_dir" hint-key="settings.storage.game_dir_hint">
        <div class="game-dir__row">
          <CoTextField class="game-dir__field" :model-value="gameDir" :placeholder="t('settings.storage.game_dir_default')" :disabled="environmentOverride" @update:model-value="emit('update:gameDir', $event)"/>
          <CoButton variant="secondary" size="sm" :disabled="environmentOverride" @click="emit('browse')"><span>{{ t("settings.storage.browse") }}</span></CoButton>
          <CoButton variant="ghost" size="sm" :disabled="environmentOverride" @click="emit('reset')"><span>{{ t("settings.storage.reset") }}</span></CoButton>
          <CoButton variant="secondary" size="sm" @click="emit('open')"><span>{{ t("settings.storage.open") }}</span></CoButton>
        </div>
      </SettingRow>
      <p class="game-dir__resolved">{{ t("settings.storage.resolved", { path: resolvedRoot || "—" }) }}</p>
      <p v-if="(gameDir.trim() && resolvedRoot !== gameDir.trim()) || environmentOverride" class="game-dir__warn">{{ t("settings.storage.overridden") }}</p>
    </SettingSection>

    <div v-if="confirmOpen" class="cleanup__confirm" role="alertdialog" aria-modal="true" aria-labelledby="cleanup-confirm-title">
      <div class="cleanup__dialog"><h3 id="cleanup-confirm-title">{{ t("settings.storage.cleanup.confirm") }}</h3><p>{{ t("settings.storage.cleanup.confirm_text", { size: formatBytes(selectedBytes) }) }}</p><div><CoButton variant="ghost" @click="confirmOpen = false">{{ t("settings.storage.cleanup.cancel") }}</CoButton><CoButton variant="danger" @click="confirmClean"><Check :size="16"/><span>{{ t("settings.storage.cleanup.confirm") }}</span></CoButton></div></div>
    </div>
  </div>
</template>

<style scoped>
.cleanup{max-width:760px}.cleanup__header{display:grid;grid-template-columns:auto 1fr;align-items:start;gap:var(--copper-space-3);margin-bottom:var(--copper-space-4)}.cleanup__header h2{margin:0 0 4px;font-size:var(--copper-font-size-lg)}.cleanup__header p{margin:0;color:var(--copper-text-secondary);font-size:var(--copper-font-size-sm)}.cleanup__list{padding:var(--copper-space-3) var(--copper-space-4);margin-bottom:var(--copper-space-5);background:var(--copper-surface);border-radius:var(--copper-radius-card);box-shadow:var(--copper-shadow-card)}.cleanup__toolbar,.cleanup__item,.cleanup__list footer{display:flex;align-items:center;gap:var(--copper-space-3)}.cleanup__toolbar{justify-content:space-between;padding:var(--copper-space-2) 0 var(--copper-space-3);color:var(--copper-text-secondary);font-size:var(--copper-font-size-sm)}.cleanup__toolbar label{display:flex;gap:8px;align-items:center}.cleanup__item{min-height:52px;padding:8px 0}.cleanup__item input,.cleanup__toolbar input{accent-color:var(--copper-accent);width:16px;height:16px;flex-shrink:0}.cleanup__name{flex:1;min-width:0;display:grid;gap:3px}.cleanup__name small{font-size:var(--copper-font-size-xs);color:var(--copper-text-secondary)}.cleanup__item strong,.cleanup__toolbar>span{font-size:var(--copper-font-size-sm);font-variant-numeric:tabular-nums}.cleanup__item--disabled{color:var(--copper-text-secondary)}.cleanup__empty{color:var(--copper-text-secondary);padding:var(--copper-space-4) 0}.cleanup__list footer{justify-content:space-between;margin-top:var(--copper-space-3);padding-top:var(--copper-space-3);background:var(--copper-surface-2);padding-inline:var(--copper-space-3);border-radius:var(--copper-radius-control);min-height:52px}.game-dir__row{display:flex;align-items:center;gap:var(--copper-space-2);flex-wrap:wrap}.game-dir__field{flex:1 1 240px;min-width:180px}.game-dir__resolved,.game-dir__warn{margin:0 var(--copper-space-2) var(--copper-space-2);font-size:var(--copper-font-size-xs);overflow-wrap:anywhere;color:var(--copper-text-secondary)}.game-dir__warn{color:var(--copper-warning,var(--copper-danger))}.cleanup__confirm{position:fixed;inset:0;z-index:1400;display:grid;place-items:center;padding:20px;background:color-mix(in srgb,var(--copper-bg) 68%,transparent)}.cleanup__dialog{width:min(420px,100%);padding:var(--copper-space-5);background:var(--copper-surface);border-radius:var(--copper-radius-card);box-shadow:var(--copper-shadow-card)}.cleanup__dialog h3{margin:0}.cleanup__dialog p{color:var(--copper-text-secondary);overflow-wrap:anywhere}.cleanup__dialog>div{display:flex;justify-content:flex-end;gap:var(--copper-space-2);flex-wrap:wrap}@media(max-width:560px){.cleanup__header{grid-template-columns:1fr}.cleanup__list footer{align-items:stretch;flex-direction:column;padding-block:var(--copper-space-3)}.game-dir__field{flex-basis:100%}}
</style>
