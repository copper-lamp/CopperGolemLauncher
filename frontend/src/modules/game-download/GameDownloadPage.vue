<script setup lang="ts">
// 游戏下载清单页：顶部「搜索 + 最新版本」卡片，下方正式版 / 测试版两个大卡片。
//
// 版式由需求钉死：
// - 顶部一张卡片：搜索框顶满左右 → 其下一行左对齐的「最新版本」文本 → 正式版、
//   测试版两张版本卡片上下排列；
// - 下方两张「全部版本」大卡片（正式版默认展开、测试版默认折叠），卡内**不再分小
//   卡片**，直接把该类型的全部版本从新到旧平铺；
// - 点任意版本卡片进入二级页面（安装在那里发起）。

import { computed, onMounted, ref, watch } from "vue";
import {
  ChevronDown,
  CircleCheck,
  Layers,
  LoaderCircle,
  RefreshCw,
  Search,
  Upload,
  X,
} from "@lucide/vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { homeLaunch } from "../../api/home";
import { useI18n } from "../../i18n";
import { usePlatform } from "../../composables/usePlatform";
import TipsRotator from "../../components/TipsRotator.vue";
import { gameImportApk } from "./api";
import { pickApkViaHost } from "./apkImport";
import { initGameDownload, useGameDownload } from "./useGameDownload";
import type { GameVersionView } from "./api";
import VersionCard from "./VersionCard.vue";

const { t } = useI18n();
const gd = useGameDownload();
const { platform } = usePlatform();

const MB_KEY = "module.game-download";

const localLoading = ref(false);
const loadError = ref<string | null>(null);
/** 搜索关键字：只过滤两个「全部版本」列表，不改动顶部最新区块。 */
const keyword = ref("");
/** 正式版大卡片是否展开（默认展开）。 */
const releaseOpen = ref(true);
/** 测试版大卡片是否展开（默认折叠；搜索命中时自动展开，否则结果会被藏起来）。 */
const previewOpen = ref(false);
/** 导入进度提示；为空表示当前无导入流程。 */
const importStatus = ref<string | null>(null);
/** 导入是否在进行中（按钮禁用 + 防重入）。 */
const importing = ref(false);

const searching = computed(() => keyword.value.trim().length > 0);

/** 按关键字过滤（版本号子串，大小写不敏感）。 */
function filter(list: GameVersionView[]): GameVersionView[] {
  const needle = keyword.value.trim().toLowerCase();
  if (!needle) return list;
  return list.filter((v) => v.game_version.toLowerCase().includes(needle));
}

const releases = computed(() => filter(gd.manifest.value?.releases ?? []));
const previews = computed(() => filter(gd.manifest.value?.previews ?? []));
const matchCount = computed(() => releases.value.length + previews.value.length);

/** 测试版是否展开：用户手动展开，或正在搜索且确有命中。 */
const previewExpanded = computed(
  () => previewOpen.value || (searching.value && previews.value.length > 0),
);

async function refresh() {
  localLoading.value = true;
  loadError.value = null;
  try {
    await gd.loadManifest(true);
  } catch (e) {
    loadError.value = String(e);
  } finally {
    localLoading.value = false;
  }
}

onMounted(async () => {
  await initGameDownload();
  // 清单未加载过则拉取；已加载（模块事件已刷新）则仅回填。
  localLoading.value = true;
  try {
    await gd.loadManifest(gd.manifest.value === null);
  } catch (e) {
    loadError.value = String(e);
  } finally {
    localLoading.value = false;
  }
});

watch(gd.manifest, () => (loadError.value = null));

/**
 * 由文件名推导实例名。
 *
 * 规则与后端 `meta::sanitize_instance_name`、安卓宿主
 * `CopperGameLayout.sanitizeInstance` 保持一致（见
 * `docs/安卓端能力差距与优先级.md` P0-2）：只保留 `A-Za-z0-9._-`，其余
 * 收敛为一个 `_`，首尾不留下划线 / 点 / 横线。
 *
 * 这里只是**给后端一个初始值**，最终实例名以 `gameImportApk` 返回的
 * `instance_name` 为准——前端不许自己算完就当结论用（两侧算法一旦漂移，
 * 就会出现「列表里看得到、点启动却说实例不存在」）。
 */
function instanceNameFrom(fileName: string): string {
  const stem = fileName.replace(/\.(apks|apk)$/i, "");
  const cleaned = stem
    .replace(/[^A-Za-z0-9._-]+/g, "_")
    .replace(/_+/g, "_")
    .replace(/^[._-]+|[._-]+$/g, "");
  if (cleaned.length === 0) return `minecraft-${Date.now()}`;
  return cleaned.slice(0, 96);
}

/**
 * 导入一个 MCBE 包并立即启动，完成「导入 → 实例管理 → 游戏启动」闭环。
 *
 * 平台差异只有一处：安卓端必须走系统文件选择器拿到 `content://` 流
 * （见 `apkImport.ts`），桌面端直接拿绝对路径。之后的导入、刷新、启动
 * 三步在所有平台完全一致。
 */
async function importApk() {
  if (importing.value) return;
  importing.value = true;
  loadError.value = null;
  try {
    let sourcePath: string;
    let displayName: string;

    if (platform.value === "android-arm64") {
      importStatus.value = t(`${MB_KEY}.import.picking`);
      const picked = await pickApkViaHost(t(`${MB_KEY}.import.cancelled`));
      sourcePath = picked.path;
      displayName = picked.displayName;
    } else {
      const selected = await openDialog({
        multiple: false,
        filters: [{ name: "Minecraft package", extensions: ["apk", "apks"] }],
      });
      if (typeof selected !== "string") return;
      sourcePath = selected;
      displayName = selected.split(/[\\/]/).pop() || "imported.apk";
    }

    importStatus.value = t(`${MB_KEY}.import.importing`);
    const info = await gameImportApk(sourcePath, instanceNameFrom(displayName));
    // 实例名以内核规整后的结果为准：这个名字同时是版本目录名与安卓宿主
    // 定位实例的键，前端不再自己推导第二遍。
    const name = info.instance_name;

    // 导入目录里现在多了这个实例，刷新清单让开始页与本页立即可见。
    await gd.loadManifest(false);
    importStatus.value = t(`${MB_KEY}.import.launching`);
    // 版本号由内核从实例元数据读取，前端不需要（也不应该）再传一遍。
    await homeLaunch(name);
    importStatus.value = t(`${MB_KEY}.import.picked`, {
      name,
      version: info.package_info.version_name,
    });
  } catch (e) {
    importStatus.value = null;
    loadError.value = t(`${MB_KEY}.import.failed`) + "：" + String(e);
  } finally {
    importing.value = false;
  }
}
</script>

<template>
  <div class="gd-page">
    <!-- 导入进度：常驻条，导入结束后保留结果直到下一次操作 -->
    <p v-if="importStatus" class="gd-page__import-status" role="status">
      <LoaderCircle v-if="importing" :size="13" class="spin" />
      <CircleCheck v-else :size="13" />
      {{ importStatus }}
    </p>
    <p v-if="loadError" class="gd-page__import-error" role="alert">{{ loadError }}</p>
    <!-- 导入 / 刷新按钮注入全局标题栏操作区 -->
    <Teleport to="#copper-titlebar-actions">
      <button
        class="gd-page__refresh"
        :title="t(`${MB_KEY}.actions.import`)"
        :disabled="importing"
        @click="importApk"
      >
        <LoaderCircle v-if="importing" :size="15" class="spin" />
        <Upload v-else :size="15" />
      </button>
      <button
        class="gd-page__refresh"
        :title="t(`${MB_KEY}.actions.refresh`)"
        :disabled="localLoading || gd.loading.value"
        @click="refresh"
      >
        <RefreshCw :size="15" :class="{ spin: localLoading || gd.loading.value }" />
      </button>
    </Teleport>

    <!-- 骨架 -->
    <div v-if="gd.loading.value && !gd.manifest.value" class="gd-page__scope">
      <div class="gd-top gd-top--skeleton">
        <div class="gd-skeleton-line" />
        <div class="gd-skeleton-line gd-skeleton-line--short" />
        <div class="gd-skeleton-line" />
      </div>
      <!-- 清单拉取期间展示内核随机提示，骨架之外再给一点可读内容 -->
      <TipsRotator compact />
    </div>

    <!-- 错误态（仅首载失败时） -->
    <div v-else-if="loadError && !gd.manifest.value" class="gd-page__state">
      <p class="gd-page__state-text">{{ t(`${MB_KEY}.error.no_manifest`) }}</p>
      <button class="gd-page__retry" @click="refresh">
        <LoaderCircle :size="14" v-if="localLoading" class="spin" />
        {{ t(`${MB_KEY}.actions.retry`) }}
      </button>
    </div>

    <!-- 空态 -->
    <div
      v-else-if="!gd.manifest.value || gd.manifest.value.releases.length + gd.manifest.value.previews.length === 0"
      class="gd-page__state"
    >
      <p class="gd-page__state-text">{{ t(`${MB_KEY}.empty`) }}</p>
      <button class="gd-page__retry" @click="refresh">{{ t(`${MB_KEY}.actions.refresh`) }}</button>
    </div>

    <!-- 内容 -->
    <div v-else class="gd-page__scope">
      <!-- 顶部卡片：搜索框 + 最新版本（正式版 / 测试版上下排列） -->
      <section class="gd-top">
        <div class="gd-search">
          <Search :size="15" class="gd-search__icon" />
          <input
            v-model="keyword"
            class="gd-search__input"
            type="search"
            :placeholder="t(`${MB_KEY}.search_placeholder`)"
          />
          <button
            v-if="searching"
            class="gd-search__clear"
            :title="t(`${MB_KEY}.actions.clear_search`)"
            @click="keyword = ''"
          >
            <X :size="14" />
          </button>
        </div>

        <p class="gd-top__label">{{ t(`${MB_KEY}.latest_section`) }}</p>

        <div class="gd-top__latest">
          <VersionCard
            v-if="gd.manifest.value.latest_release"
            :version="gd.manifest.value.latest_release"
          />
          <p v-else class="gd-top__empty">{{ t(`${MB_KEY}.empty`) }}</p>
          <VersionCard
            v-if="gd.manifest.value.latest_preview"
            :version="gd.manifest.value.latest_preview"
          />
        </div>
      </section>

      <!-- 全部版本：正式版（默认展开） -->
      <section class="gd-group">
        <button class="gd-group__head" @click="releaseOpen = !releaseOpen">
          <ChevronDown
            :size="15"
            class="gd-group__chevron"
            :class="{ 'gd-group__chevron--open': releaseOpen }"
          />
          <Layers :size="15" />
          <span class="gd-group__title">{{ t(`${MB_KEY}.kind.release`) }}</span>
          <span class="gd-group__count">
            {{ t(`${MB_KEY}.versionsCount`, { n: gd.manifest.value.releases.length }) }}
          </span>
        </button>
        <div v-show="releaseOpen" class="gd-group__body">
          <div v-if="releases.length === 0" class="gd-group__none">
            {{ searching ? t(`${MB_KEY}.no_match`) : t(`${MB_KEY}.empty`) }}
          </div>
          <div v-else class="gd-grid">
            <VersionCard v-for="version in releases" :key="version.id" :version="version" />
          </div>
        </div>
      </section>

      <!-- 全部版本：测试版（默认折叠，可展开） -->
      <section class="gd-group">
        <button class="gd-group__head" @click="previewOpen = !previewOpen">
          <ChevronDown
            :size="15"
            class="gd-group__chevron"
            :class="{ 'gd-group__chevron--open': previewExpanded }"
          />
          <Layers :size="15" />
          <span class="gd-group__title">{{ t(`${MB_KEY}.kind.preview`) }}</span>
          <span class="gd-group__count">
            {{ t(`${MB_KEY}.versionsCount`, { n: gd.manifest.value.previews.length }) }}
          </span>
        </button>
        <div v-show="previewExpanded" class="gd-group__body">
          <div v-if="previews.length === 0" class="gd-group__none">
            {{ searching ? t(`${MB_KEY}.no_match`) : t(`${MB_KEY}.empty`) }}
          </div>
          <div v-else class="gd-grid">
            <VersionCard v-for="version in previews" :key="version.id" :version="version" />
          </div>
        </div>
      </section>

      <!-- 搜索命中数：给「搜到了但没有可见结果」一个明确交代 -->
      <p v-if="searching" class="gd-page__match">
        {{ t(`${MB_KEY}.match_count`, { n: matchCount }) }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.gd-page {
  height: 100%;
  padding: var(--copper-space-5);
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}

.gd-page__import-status {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0 0 var(--copper-space-2);
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.gd-page__import-error {
  margin: 0 0 var(--copper-space-2);
  font-size: var(--copper-font-size-sm);
  color: var(--copper-danger);
}

.gd-page__refresh {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.gd-page__refresh:hover:not(:disabled) {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.gd-page__refresh:disabled {
  opacity: 0.5;
  cursor: default;
}

.gd-page__scope {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-4);
}

.gd-page__state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-6);
}

.gd-page__state-text {
  color: var(--copper-text-secondary);
}

.gd-page__retry {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.gd-page__retry:hover {
  background: var(--copper-surface-2);
}

.gd-page__match {
  display: flex;
  align-items: center;
  gap: 5px;
  margin: 0;
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
}

/* ---------------------------------------------------------------- 顶部卡片 */

.gd-top {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  padding: var(--copper-space-4);
  background: var(--copper-surface);
  border-radius: var(--copper-radius-lg);
}

.gd-top__label {
  margin: 0;
  /* 需求：搜索框下方的**左侧**一行「最新版本」文本。 */
  align-self: flex-start;
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.gd-top__latest {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-1);
}

.gd-top__empty {
  margin: 0;
  padding: var(--copper-space-2) var(--copper-space-3);
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

.gd-search {
  position: relative;
  display: flex;
  align-items: center;
  width: 100%;
}

.gd-search__icon {
  position: absolute;
  left: var(--copper-space-3);
  color: var(--copper-text-disabled);
  pointer-events: none;
}

.gd-search__input {
  width: 100%;
  height: var(--copper-control-h);
  padding: 0 var(--copper-space-6) 0 calc(var(--copper-space-3) + 22px);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  font-family: inherit;
  transition:
    border-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.gd-search__input::-webkit-search-cancel-button {
  display: none;
}

.gd-search__input:hover {
  border-color: color-mix(in srgb, var(--copper-accent) 50%, var(--copper-border));
}

.gd-search__input:focus-visible {
  outline: none;
  border-color: var(--copper-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--copper-accent) 22%, transparent);
}

.gd-search__clear {
  position: absolute;
  right: var(--copper-space-2);
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.gd-search__clear:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

/* ---------------------------------------------------------------- 大卡片 */

.gd-group {
  background: var(--copper-surface);
  border-radius: var(--copper-radius-lg);
  overflow: hidden;
}

.gd-group__head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  width: 100%;
  padding: var(--copper-space-3);
  border: none;
  background: transparent;
  color: var(--copper-text);
  font-size: var(--copper-font-size-lg);
  font-family: inherit;
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.gd-group__head:hover {
  background: var(--copper-surface-2);
}

.gd-group__title {
  font-weight: 700;
}

.gd-group__count {
  margin-left: auto;
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-secondary);
}

.gd-group__chevron {
  color: var(--copper-text-secondary);
  transition: transform var(--copper-duration) var(--copper-easing);
}

.gd-group__chevron--open {
  transform: rotate(180deg);
}

.gd-group__body {
  padding: 0 var(--copper-space-3) var(--copper-space-3);
}

.gd-group__none {
  padding: var(--copper-space-3);
  font-size: var(--copper-font-size-sm);
  color: var(--copper-text-secondary);
}

/* 平铺：卡内不再分小卡片，全部版本直接铺开。 */
.gd-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  gap: var(--copper-space-1);
}

/* ---------------------------------------------------------------- 骨架 */

.gd-top--skeleton .gd-skeleton-line {
  height: 14px;
  border-radius: var(--copper-radius-sm);
  background: var(--copper-surface-3);
}

.gd-top--skeleton .gd-skeleton-line--short {
  width: 60%;
}

.spin {
  animation: spin 1.2s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
