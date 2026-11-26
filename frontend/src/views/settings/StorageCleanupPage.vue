<script setup lang="ts">
// 清理储存（二级页，真实路由 /settings/storage/cleanup）。
//
// 这里只做两件事：勾选要清理的分类、确认后交给后端删除。所有分类都可清理，
// 不能清理的东西（游戏本体、存档）根本不会出现在这里。

import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { Check, Trash2 } from "@lucide/vue";

import CoButton from "../../components/ui/CoButton.vue";
import { useI18n } from "../../i18n";
import { formatBytes } from "../../api/download";
import { useStorageScan } from "../../composables/useStorageScan";
import { showToast } from "../../composables/useToast";
import { storageCategoryColor, storageCategoryMeta } from "../../services/storage-categories";
import { STORAGE_GROUPS } from "../../services/storage-categories";

const { t } = useI18n();
const router = useRouter();
const scan = useStorageScan();

const selected = ref<string[]>([]);
const confirmOpen = ref(false);
const cleaning = ref(false);
/** 已经出现过（有统计值）的分类：用户取消勾选后不会被后续事件重新选中。 */
const seen = new Set<string>();

const scanning = computed(() => scan.scanning.value);
const categories = computed(() => scan.categories.value);
const cleanable = computed(() => categories.value.filter((item) => item.cleanable && item.bytes !== null));

const groups = computed(() =>
  STORAGE_GROUPS.map((group) => ({
    id: group.id,
    rows: group.ids.map((id) => {
      const scanItem = categories.value.find((item) => item.id === id);
      const meta = storageCategoryMeta(id);
      return {
        id,
        color: storageCategoryColor(meta?.tone ?? 1),
        bytes: scanItem?.bytes ?? null,
        state: scanItem?.state ?? "pending",
        cleanable: scanItem?.cleanable ?? false,
      };
    }),
  })),
);

const allSelected = computed(
  () => cleanable.value.length > 0 && cleanable.value.every((item) => selected.value.includes(item.id)),
);
const selectedBytes = computed(() =>
  cleanable.value.filter((item) => selected.value.includes(item.id)).reduce((sum, item) => sum + (item.bytes ?? 0), 0),
);
const blocked = computed(() => scanning.value || cleaning.value || !cleanable.value.length);

// 分类结果是逐条到达的：第一次出现的分类默认勾选，之后用户的手动取消要保留。
watch(
  cleanable,
  (items) => {
    const next = [...selected.value];
    for (const item of items) {
      if (!seen.has(item.id)) {
        seen.add(item.id);
        next.push(item.id);
      }
    }
    selected.value = next;
  },
  { immediate: true },
);

function toggle(id: string) {
  selected.value = selected.value.includes(id)
    ? selected.value.filter((item) => item !== id)
    : [...selected.value, id];
}

function toggleAll() {
  selected.value = allSelected.value ? [] : cleanable.value.map((item) => item.id);
}

async function runClean() {
  if (blocked.value || !selected.value.length) return;
  confirmOpen.value = false;
  cleaning.value = true;
  try {
    const freed = await scan.clean([...selected.value]);
    showToast(t("settings.storage.cleanup.success", { size: formatBytes(freed) }), "success");
    selected.value = [];
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    cleaning.value = false;
  }
}

/** 返回一级页：有历史时用 back 保留滚动位置，否则直接进入储存位置。 */
function goBack() {
  if (window.history.length > 1) router.back();
  else void router.push({ name: "settings-storage" });
}
</script>

<template>
  <div class="cleanup">
    <header class="cleanup__header">
      <div class="cleanup__intro">
        <h2>{{ t("settings.storage.cleanup.title") }}</h2>
        <p>{{ t("settings.storage.cleanup.desc") }}</p>
      </div>
      <CoButton variant="ghost" size="sm" @click="goBack"><span>{{ t("settings.storage.cleanup.back") }}</span></CoButton>
    </header>

    <section class="cleanup__list">
      <p v-if="scanning" class="cleanup__notice" role="status">{{ t("settings.storage.cleanup.scanning") }}</p>
      <div class="cleanup__toolbar">
        <label>
          <input type="checkbox" :checked="allSelected" :disabled="blocked" @change="toggleAll" />
          <span>{{ t("settings.storage.cleanup.select_all") }}</span>
        </label>
        <span>{{ t("settings.storage.cleanup.selected", { count: selected.length }) }}</span>
      </div>

      <section v-for="group in groups" :key="group.id" class="cleanup__group">
        <h3>{{ t("settings.storage.usage.group." + group.id) }}</h3>
        <label
          v-for="row in group.rows"
          :key="row.id"
          class="cleanup__item"
          :class="{ 'is-disabled': row.bytes === null }"
        >
          <input
            type="checkbox"
            :checked="selected.includes(row.id)"
            :disabled="blocked || row.bytes === null"
            @change="toggle(row.id)"
          />
          <i class="cleanup__swatch" :style="{ background: row.color }" aria-hidden="true" />
          <span class="cleanup__name">
            <span>{{ t("settings.storage.usage.category." + row.id) }}</span>
            <small>{{ t("settings.storage.usage.hint." + row.id) }}</small>
          </span>
          <strong v-if="row.bytes !== null">{{ formatBytes(row.bytes) }}</strong>
          <span v-else class="cleanup__unknown">{{ t("settings.storage.usage.pendingValue") }}</span>
        </label>
      </section>

      <footer>
        <span>{{ t("settings.storage.cleanup.free") }} <strong>{{ formatBytes(selectedBytes) }}</strong></span>
        <CoButton variant="danger" :disabled="blocked || !selected.length" @click="confirmOpen = true">
          <Trash2 :size="16" /><span>{{ t("settings.storage.cleanup.confirm") }}</span>
        </CoButton>
      </footer>
    </section>

    <div v-if="confirmOpen" class="cleanup__confirm" role="alertdialog" aria-modal="true" aria-labelledby="cleanup-confirm-title">
      <div class="cleanup__dialog">
        <h3 id="cleanup-confirm-title">{{ t("settings.storage.cleanup.confirm") }}</h3>
        <p>{{ t("settings.storage.cleanup.confirm_text", { size: formatBytes(selectedBytes) }) }}</p>
        <div class="cleanup__dialog-actions">
          <CoButton variant="ghost" @click="confirmOpen = false">{{ t("settings.storage.cleanup.cancel") }}</CoButton>
          <CoButton variant="danger" @click="runClean"><Check :size="16" /><span>{{ t("settings.storage.cleanup.confirm") }}</span></CoButton>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.cleanup{display:grid;gap:var(--copper-space-4);max-width:840px}
.cleanup__header{display:flex;align-items:flex-start;justify-content:space-between;gap:var(--copper-space-3)}
.cleanup__intro h2{margin:0 0 4px;font-size:var(--copper-font-size-lg);font-weight:600}
.cleanup__intro p{margin:0;color:var(--copper-text-secondary);font-size:var(--copper-font-size-sm)}
.cleanup__list{padding:var(--copper-space-3) var(--copper-space-4) var(--copper-space-4);background:var(--copper-surface);border-radius:var(--copper-radius-card);box-shadow:var(--copper-shadow-card)}
.cleanup__notice{margin:0 0 var(--copper-space-2);padding:8px 10px;border-radius:var(--copper-radius-sm);background:color-mix(in srgb,var(--copper-accent) 12%,transparent);color:var(--copper-text-secondary);font-size:var(--copper-font-size-xs)}
.cleanup__toolbar{display:flex;align-items:center;justify-content:space-between;gap:var(--copper-space-3);padding:var(--copper-space-2) 0 var(--copper-space-3);color:var(--copper-text-secondary);font-size:var(--copper-font-size-sm)}
.cleanup__toolbar label{display:flex;gap:8px;align-items:center}
.cleanup__group{display:grid;gap:1px}
.cleanup__group h3{margin:var(--copper-space-2) 0 2px;font-size:var(--copper-font-size-xs);font-weight:600;letter-spacing:.04em;text-transform:uppercase;color:var(--copper-text-secondary);opacity:.72}
.cleanup__item{display:flex;align-items:center;gap:var(--copper-space-3);min-height:56px;padding:6px 8px;margin:0 -8px;border-radius:var(--copper-radius-sm);cursor:pointer;transition:background-color var(--copper-duration-fast) var(--copper-easing)}
.cleanup__item:hover{background:var(--copper-hover)}
.cleanup__item.is-disabled{cursor:default}
.cleanup__item.is-disabled:hover{background:transparent}
.cleanup__item input,.cleanup__toolbar input{accent-color:var(--copper-accent);width:16px;height:16px;flex-shrink:0}
.cleanup__swatch{width:10px;height:10px;border-radius:3px;flex:0 0 auto}
.cleanup__name{flex:1;min-width:0;display:grid;gap:2px}
.cleanup__name>span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.cleanup__name small{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:var(--copper-font-size-xs);color:var(--copper-text-secondary);opacity:.82}
.cleanup__item strong{font-size:var(--copper-font-size-sm);font-variant-numeric:tabular-nums;font-weight:550}
.cleanup__unknown{font-size:var(--copper-font-size-xs);color:var(--copper-text-secondary)}
.cleanup__list footer{display:flex;align-items:center;justify-content:space-between;gap:var(--copper-space-3);margin-top:var(--copper-space-3);padding:var(--copper-space-2) var(--copper-space-3);min-height:56px;background:var(--copper-surface-2);border-radius:var(--copper-radius-control)}
.cleanup__list footer span{font-size:var(--copper-font-size-sm);color:var(--copper-text-secondary)}
.cleanup__list footer strong{color:var(--copper-text);font-variant-numeric:tabular-nums}
.cleanup__confirm{position:fixed;inset:0;z-index:60;display:grid;place-items:center;padding:var(--copper-space-4);background:color-mix(in srgb,var(--copper-bg) 72%,transparent);backdrop-filter:blur(4px)}
.cleanup__dialog{width:min(420px,100%);padding:var(--copper-space-5);background:var(--copper-surface);border-radius:var(--copper-radius-card);box-shadow:var(--copper-shadow-card);display:grid;gap:var(--copper-space-3)}
.cleanup__dialog h3{margin:0;font-size:var(--copper-font-size-md)}
.cleanup__dialog p{margin:0;color:var(--copper-text-secondary);font-size:var(--copper-font-size-sm);line-height:1.6}
.cleanup__dialog-actions{display:flex;justify-content:flex-end;gap:var(--copper-space-2)}
</style>
