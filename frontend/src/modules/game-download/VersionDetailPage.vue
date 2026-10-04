<script setup lang="ts">
// 版本二级页面：选加载器 / 客户端 → 安装（弹窗命名实例）。
//
// 版式由需求钉死：顶部是版本图标与版本号（无边框），下面是「加载器」「客户端」两个
// 下拉框，页面正下方居中一个安装按钮。这里**不列实例、不画进度**：一次安装产出一个
// 实例，进度统一在下载中心看（下载引擎是任务的事实源），本页只负责发起。

import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { AlertTriangle, Gamepad2, PackagePlus } from "@lucide/vue";

import CoButton from "../../components/ui/CoButton.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import { useI18n } from "../../i18n";
import InstanceNameDialog from "./InstanceNameDialog.vue";
import type { GameVersionView } from "./api";
import { initGameDownload, useGameDownload } from "./useGameDownload";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const gd = useGameDownload();

const MB_KEY = "module.game-download";

const id = computed(() => String(route.params.id ?? ""));

/** 从清单视图定位当前版本（正式版 / 测试版两张平表 + 顶部最新卡）。 */
const version = computed<GameVersionView | null>(() => {
  const m = gd.manifest.value;
  if (!m) return null;
  const latest = [m.latest_release, m.latest_preview].find((v) => v?.id === id.value);
  if (latest) return latest;
  return [...m.releases, ...m.previews].find((v) => v.id === id.value) ?? null;
});

/** 加载器清单（本机 lipd 可用性 + 该游戏版本的全部 LeviLamina 候选）。 */
const loaders = computed(() => gd.loaderCatalogs.value[id.value] ?? null);
const lipAvailable = computed(() => loaders.value?.lip_available ?? true);
/**
 * 下拉里只列**可用**的加载器。
 *
 * 不兼容的版本在数据里仍然存在（列表页徽标要看全量），但对用户而言「选一个装不上的
 * 版本」没有意义：后端按索引声明的平台依赖严格判定，放进去只会让安装走到一半才由
 * lipd 报依赖冲突，那是最差的一种反馈。
 */
const compatibleLoaders = computed(() => (loaders.value?.loaders ?? []).filter((l) => l.compatible));

const loader = ref("");
const client = ref("");

const loaderChoices = computed(() => [
  { value: "", label: t(`${MB_KEY}.loader_none`) },
  ...compatibleLoaders.value.map((l) => ({
    value: l.version,
    label: `LeviLamina ${l.version}`,
  })),
]);

/** 客户端下拉：当前没有收录任何客户端 dll，保持空表 + 占位项。 */
const clientChoices = computed(() => [{ value: "", label: t(`${MB_KEY}.client_empty`) }]);

/** 选中加载器声明的平台依赖（给用户一个「为什么可选的是这几个」的交代）。 */
const requirement = computed(
  () => compatibleLoaders.value.find((l) => l.version === loader.value)?.requirement ?? null,
);

const dialogOpen = ref(false);

function openDialog() {
  if (!version.value) return;
  dialogOpen.value = true;
}

onMounted(async () => {
  await initGameDownload();
  await gd.loadManifest(gd.manifest.value === null);
  await gd.loadLoaders(id.value);
});

watch(id, async (next) => {
  loader.value = "";
  client.value = "";
  if (next) await gd.loadLoaders(next);
});

/** 返回列表（未找到版本等兜底引导）。 */
function goBack() {
  void router.push("/game-download");
}
</script>

<template>
  <div class="gd-detail">
    <!-- 未找到版本 -->
    <div v-if="!version" class="gd-detail__state">
      <p class="gd-detail__state-text">{{ t(`${MB_KEY}.error.not_found`) }}</p>
      <CoButton variant="secondary" @click="goBack">{{ t(`${MB_KEY}.actions.back`) }}</CoButton>
    </div>

    <template v-else>
      <div class="gd-detail__column">
        <!-- 顶部：版本图标与版本号，无边框 -->
        <header class="gd-detail__hero">
          <span class="gd-detail__icon" aria-hidden="true">
            <Gamepad2 :size="30" />
          </span>
          <div class="gd-detail__hero-text">
            <span class="gd-detail__name">{{ version.game_version }}</span>
            <span class="gd-detail__badges">
              <span class="gd-detail__badge" :class="`gd-detail__badge--${version.kind}`">
                {{ t(`${MB_KEY}.kind.${version.kind}`) }}
              </span>
              <span v-if="version.has_loader" class="gd-detail__badge gd-detail__badge--loader">
                LeviLamina
              </span>
            </span>
          </div>
        </header>

        <!-- 两个下拉框：加载器 / 客户端 -->
        <div class="gd-detail__form">
          <label class="gd-detail__field">
            <span class="gd-detail__label">{{ t(`${MB_KEY}.loader_label`) }}</span>
            <CoSelect v-model="loader" :options="loaderChoices" />
            <span v-if="requirement" class="gd-detail__hint">
              {{ t(`${MB_KEY}.loader_requirement`, { requirement }) }}
            </span>
            <span v-else-if="compatibleLoaders.length === 0" class="gd-detail__hint">
              {{ t(`${MB_KEY}.loader_unavailable`) }}
            </span>
          </label>

          <label class="gd-detail__field">
            <span class="gd-detail__label">{{ t(`${MB_KEY}.client_label`) }}</span>
            <CoSelect v-model="client" :options="clientChoices" disabled />
            <span class="gd-detail__hint">{{ t(`${MB_KEY}.client_hint`) }}</span>
          </label>

          <!-- lipd 缺失：装完游戏也补不上加载器，必须提前说清楚 -->
          <p v-if="loader && !lipAvailable" class="gd-detail__warn" role="alert">
            <AlertTriangle :size="14" />
            {{ t(`${MB_KEY}.lip_missing`) }}
          </p>
        </div>

        <div class="gd-detail__spacer" />

        <!-- 正下方居中的安装按钮 -->
        <div class="gd-detail__actions">
          <CoButton variant="primary" @click="openDialog">
            <PackagePlus :size="16" />
            {{ t(`${MB_KEY}.actions.install`) }}
          </CoButton>
          <span class="gd-detail__actions-hint">{{ t(`${MB_KEY}.progress_hint`) }}</span>
        </div>
      </div>

      <InstanceNameDialog v-model:open="dialogOpen" :version="version" :loader="loader || null" />
    </template>
  </div>
</template>

<style scoped>
.gd-detail {
  height: 100%;
  padding: var(--copper-space-5);
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}

.gd-detail__state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-3);
}

.gd-detail__state-text {
  color: var(--copper-text-secondary);
}

.gd-detail__column {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-5);
  width: 100%;
  max-width: 520px;
  margin: 0 auto;
}

/* 无边框头部：只有图标与文字，不做卡片。 */
.gd-detail__hero {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
}

.gd-detail__icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 56px;
  height: 56px;
  flex-shrink: 0;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-accent);
}

.gd-detail__hero-text {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
  min-width: 0;
}

.gd-detail__name {
  font-size: var(--copper-font-size-xl);
  font-weight: 700;
}

.gd-detail__badges {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
  flex-wrap: wrap;
}

.gd-detail__badge {
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  font-size: var(--copper-font-size-xs);
  line-height: 1.5;
  border: 1px solid transparent;
}

.gd-detail__badge--release {
  color: var(--copper-badge-release);
  background: var(--copper-badge-release-bg);
  border-color: var(--copper-badge-release-border);
}

.gd-detail__badge--preview {
  color: var(--copper-badge-alpha);
  background: var(--copper-badge-alpha-bg);
  border-color: var(--copper-badge-alpha-border);
}

.gd-detail__badge--loader {
  color: var(--copper-badge-ll-mod);
  background: var(--copper-badge-ll-mod-bg);
  border-color: var(--copper-badge-ll-mod-border);
}

.gd-detail__form {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-4);
}

.gd-detail__field {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
  align-items: flex-start;
}

.gd-detail__label {
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.gd-detail__hint {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

.gd-detail__warn {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  font-size: var(--copper-font-size-sm);
  color: var(--copper-warning);
}

.gd-detail__spacer {
  flex: 1;
  min-height: var(--copper-space-4);
}

.gd-detail__actions {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-2);
  padding-bottom: var(--copper-space-2);
}

.gd-detail__actions-hint {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}
</style>
