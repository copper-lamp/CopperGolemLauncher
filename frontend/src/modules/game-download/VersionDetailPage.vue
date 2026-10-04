<script setup lang="ts">
// 版本二级页面：选加载器 / 客户端 → 安装（弹窗命名实例）。
//
// 版式由需求钉死：顶部是版本图标与版本号（无边框），下面是「加载器」「客户端」两个
// 下拉框（自绘组件，折叠态显示「标签 + 未选择」，展开向下弹出），页面正下方居中一个
// 安装按钮。左右只留很小的边距，内容尽量铺满；除必要告警外不写说明性文字。
//
// 这里**不列实例、不画进度**：一次安装产出一个实例，进度统一在下载中心看（下载引擎
// 是任务的事实源），本页只负责发起。

import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { AlertTriangle, Ban, Gamepad2, PackagePlus, Puzzle } from "@lucide/vue";

import CoButton from "../../components/ui/CoButton.vue";
import CoDropdown, { type DropdownOption } from "../../components/ui/CoDropdown.vue";
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

/** 加载器清单（本机 lipd 可用性 + 该游戏版本的 LeviLamina 候选，已按版本库筛过）。 */
const loaders = computed(() => gd.loaderCatalogs.value[id.value] ?? null);
const lipAvailable = computed(() => loaders.value?.lip_available ?? true);
const availableLoaders = computed(() => loaders.value?.loaders ?? []);

const loader = ref("");
const client = ref("");

/** 加载器选项：首项是「不使用加载器」，其余为版本库给出的可用 LeviLamina。 */
const loaderChoices = computed<DropdownOption[]>(() => [
  { value: "", label: t(`${MB_KEY}.loader_none`), icon: Ban },
  ...availableLoaders.value.map((option) => ({
    value: option.version,
    label: `LeviLamina ${option.version}`,
    icon: Puzzle,
  })),
]);

/** 客户端下拉：当前没有收录任何客户端 dll，展开后由组件给出空态说明。 */
const clientChoices = computed<DropdownOption[]>(() => []);

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
          <span class="gd-detail__name">{{ version.game_version }}</span>
          <span class="gd-detail__badge" :class="`gd-detail__badge--${version.kind}`">
            {{ t(`${MB_KEY}.kind.${version.kind}`) }}
          </span>
          <span v-if="version.has_loader" class="gd-detail__badge gd-detail__badge--loader">
            LeviLamina
          </span>
        </header>

        <!-- 两个下拉框：加载器 / 客户端。整块放进滚动容器：
             下拉框是「自身变长」的，展开后内容可能比一屏还高；不套滚动容器，
             页面就会被顶长、把底部安装按钮推到屏幕外。 -->
        <div class="gd-detail__form">
          <div class="gd-detail__form-scroll">
            <CoDropdown
              v-model="loader"
              :label="t(`${MB_KEY}.loader_label`)"
              :placeholder="t(`${MB_KEY}.select_none`)"
              :options="loaderChoices"
              :empty-text="t(`${MB_KEY}.loader_unavailable`)"
            />
            <CoDropdown
              v-model="client"
              :label="t(`${MB_KEY}.client_label`)"
              :placeholder="t(`${MB_KEY}.select_none`)"
              :options="clientChoices"
              :empty-text="t(`${MB_KEY}.client_empty`)"
            />
          </div>

          <!-- lipd 缺失：装完游戏也补不上加载器，必须提前说清楚（本页唯一的告警文字） -->
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
        </div>
      </div>

      <InstanceNameDialog v-model:open="dialogOpen" :version="version" :loader="loader || null" />
    </template>
  </div>
</template>

<style scoped>
/* 左右只留很小的边距：需求点名不要大片留白。 */
.gd-detail {
  height: 100%;
  padding: var(--copper-space-4);
  overflow: hidden;
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
  /* 页面本体不滚动：只有下拉框那块滚动，底部安装按钮永远留在屏幕内。 */
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-5);
  width: 100%;
}

/* 无边框头部：图标、版本号、徽标并排一行。 */
.gd-detail__hero {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  flex-wrap: wrap;
  /* 头部与底部按钮都不参与收缩：可压缩的只有下拉框那块。 */
  flex-shrink: 0;
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

.gd-detail__name {
  font-size: var(--copper-font-size-xl);
  font-weight: 700;
  letter-spacing: 0.2px;
}

.gd-detail__badge {
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  font-size: var(--copper-font-size-xs);
  line-height: 1.6;
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
  gap: var(--copper-space-3);
  width: 100%;
  /* 收缩到内容与可用空间的较小者：内容多则这块自己滚动，页面不被顶长。 */
  min-height: 0;
  flex: 0 1 auto;
}

/* 下拉框自身变长后可能超出视口，这一层负责滚动。 */
.gd-detail__form-scroll {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  min-height: 0;
  overflow-y: auto;
  /* 滚动条贴着卡片边缘，右侧留一点内边距避免压住圆角。 */
  padding-right: var(--copper-space-1);
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
  justify-content: center;
  padding-bottom: var(--copper-space-2);
  flex-shrink: 0;
}
</style>
