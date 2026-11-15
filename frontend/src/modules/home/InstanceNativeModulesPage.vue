<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { CircleCheck, CircleSlash, Package, Trash2 } from "@lucide/vue";
import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { useVersionInstance } from "./useVersionInstance";
import { androidNativeModulesList, androidNativeModuleRemove, androidNativeModuleSetEnabled, type ModView } from "../../api/home";
import CoContentIcon from "../../components/ui/CoContentIcon.vue";
import InstanceSearchBox from "./InstanceSearchBox.vue";

const { t } = useI18n();
const { currentName } = useVersionInstance();
const mods = ref<ModView[]>([]);
const loading = ref(false);
const errorText = ref("");
const search = ref("");
const filter = ref<"all" | "enabled" | "disabled">("all");
const visibleMods = computed(() => {
  const query = search.value.trim().toLowerCase();
  return mods.value.filter((mod) => {
    if (filter.value === "enabled" && !mod.enabled) return false;
    if (filter.value === "disabled" && mod.enabled) return false;
    return !query || [mod.name, mod.folder, mod.mod_type, mod.author, mod.description].some((value) => value.toLowerCase().includes(query));
  });
});

async function refresh() {
  const name = currentName.value;
  if (!name) { mods.value = []; return; }
  loading.value = true; errorText.value = "";
  try {
    const result = await androidNativeModulesList(name);
    if (name === currentName.value) mods.value = result;
  } catch (error) {
    if (name === currentName.value) { errorText.value = String(error); mods.value = []; }
  } finally {
    if (name === currentName.value) loading.value = false;
  }
}
watch(currentName, () => void refresh(), { immediate: true });

async function toggle(mod: ModView) {
  try { await androidNativeModuleSetEnabled(currentName.value, mod.folder, !mod.enabled); await refresh(); }
  catch (error) { showToast(String(error), "error"); }
}
async function remove(mod: ModView) {
  if (!window.confirm(t("module.home.native_modules.remove_confirm", { name: mod.name }))) return;
  try { await androidNativeModuleRemove(currentName.value, mod.folder); showToast(t("module.home.native_modules.removed"), "success"); await refresh(); }
  catch (error) { showToast(String(error), "error"); }
}
</script>

<template>
  <div class="native-modules">
    <section class="native-modules__main">
      <div class="native-modules__filters" role="tablist">
        <button v-for="value in (['all', 'enabled', 'disabled'] as const)" :key="value" type="button" role="tab" :aria-selected="filter === value" :class="['chip', { 'chip--active': filter === value }]" @click="filter = value">
          <span>{{ t(`module.home.native_modules.filters.${value}`) }}</span><span class="count">{{ value === 'all' ? mods.length : mods.filter(m => m.enabled === (value === 'enabled')).length }}</span>
        </button>
      </div>
      <div class="native-modules__body">
        <div v-if="loading" class="state">{{ t("common.loading") }}</div>
        <div v-else-if="errorText" class="state">{{ errorText }}</div>
        <div v-else-if="visibleMods.length === 0" class="state"><Package :size="22" :stroke-width="1.5" /><span>{{ search.trim() ? t("module.home.native_modules.empty_search") : t("module.home.native_modules.empty") }}</span></div>
        <div v-else class="list" role="list">
          <div v-for="mod in visibleMods" :key="mod.folder" :class="['row', { 'row--off': !mod.enabled }]">
            <span class="icon"><CoContentIcon :name="mod.name" :src="mod.icon" /></span>
            <span class="text"><strong>{{ mod.name }}</strong><small>{{ mod.description.trim() || (mod.author.trim() ? `${mod.mod_type} · ${mod.author}` : mod.mod_type) }}</small></span>
            <span class="version"><span v-if="!mod.enabled">{{ t("module.home.native_modules.disabled") }}</span>{{ mod.version || '—' }}</span>
            <span class="actions">
              <button type="button" class="action" :title="mod.enabled ? t('module.home.native_modules.disable') : t('module.home.native_modules.enable')" @click="toggle(mod)"><CircleSlash v-if="mod.enabled" :size="16" /><CircleCheck v-else :size="16" /></button>
              <button type="button" class="action action--danger" :title="t('module.home.native_modules.remove')" @click="remove(mod)"><Trash2 :size="16" /></button>
            </span>
          </div>
        </div>
      </div>
    </section>
    <aside class="native-modules__aside"><InstanceSearchBox v-model="search" :placeholder="t('module.home.native_modules.search_placeholder')" /></aside>
  </div>
</template>

<style scoped>
.native-modules { display:grid; grid-template-columns:minmax(0,6fr) minmax(0,4fr); gap:var(--copper-space-4); height:100%; min-height:0; padding:var(--copper-space-4); }
.native-modules__main,.native-modules__aside { display:flex; flex-direction:column; min-width:0; min-height:0; gap:var(--copper-space-3); }
.native-modules__filters { display:flex; flex-wrap:wrap; gap:var(--copper-space-1); }
.chip { display:inline-flex; align-items:center; gap:6px; height:30px; padding:0 var(--copper-space-3); border:0; border-radius:var(--copper-radius-full); background:transparent; color:var(--copper-text-secondary); font:inherit; cursor:pointer; }
.chip:hover { background:var(--copper-surface-2); color:var(--copper-text); }
.chip--active { background:color-mix(in srgb,var(--copper-accent) 14%,transparent); color:color-mix(in srgb,var(--copper-accent) 85%,var(--copper-text)); }
.count { color:var(--copper-text-disabled); font-size:var(--copper-font-size-xs); }
.native-modules__body { flex:1; min-height:0; display:flex; flex-direction:column; }
.state { flex:1; display:flex; flex-direction:column; align-items:center; justify-content:center; gap:var(--copper-space-2); color:var(--copper-text-disabled); font-size:var(--copper-font-size-sm); text-align:center; }
.list { flex:1; min-height:0; overflow-y:auto; display:flex; flex-direction:column; gap:1px; }
.row { display:flex; align-items:center; gap:var(--copper-space-3); min-height:56px; padding:var(--copper-space-2) var(--copper-space-3); border-radius:var(--copper-radius-md); transition:background-color var(--copper-duration-fast) var(--copper-easing); }
.row:hover { background:var(--copper-surface-2); }
.row--off { opacity:.75; }
.icon { flex:none; display:grid; place-items:center; width:34px; height:34px; overflow:hidden; border-radius:var(--copper-radius-sm); background:color-mix(in srgb,var(--copper-text) 6%,transparent); }
.icon :deep(img) { width:100%; height:100%; object-fit:cover; }
.text { flex:1; min-width:0; display:flex; flex-direction:column; gap:2px; }
.text strong,.text small { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.text strong { font-size:var(--copper-font-size-md); font-weight:500; }
.text small,.version { color:var(--copper-text-disabled); font-size:var(--copper-font-size-xs); }
.version { display:flex; gap:var(--copper-space-2); flex:none; }
.actions { display:flex; gap:2px; flex:none; opacity:0; pointer-events:none; transition:opacity var(--copper-duration-fast) var(--copper-easing); }
.row:hover .actions,.row:focus-within .actions { opacity:1; pointer-events:auto; }
.action { display:grid; place-items:center; width:28px; height:28px; border:0; border-radius:var(--copper-radius-sm); background:transparent; color:var(--copper-text-secondary); cursor:pointer; }
.action:hover { background:var(--copper-hover); color:var(--copper-text); }
.action--danger:hover { background:color-mix(in srgb,var(--copper-danger) 14%,transparent); color:var(--copper-danger); }
@media(max-width:900px) { .native-modules { grid-template-columns:minmax(0,1fr); grid-template-rows:minmax(0,1fr) auto; overflow-y:auto; } .native-modules__aside { overflow:visible; } }
</style>
