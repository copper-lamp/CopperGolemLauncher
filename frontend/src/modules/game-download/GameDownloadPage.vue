<script setup lang="ts">
// 游戏下载清单页：最新正式 / 最新预览置顶 + 按大版本分组展示全部版本。
//
// 数据来自后端 `ManifestView`：`latest_release/latest_preview` 置顶高亮，
// `groups` 按大版本降序分组。行内下载 / 进度 / 取消复用 `VersionRow`。

import { onMounted, ref, watch } from "vue";
import { RefreshCw, Rocket, LoaderCircle, Layers } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { initGameDownload, useGameDownload } from "./useGameDownload";
import VersionRow from "./VersionRow.vue";
import type { GameVersionView } from "./api";

const { t } = useI18n();
const gd = useGameDownload();

const MB_KEY = "module.game-download";

const localLoading = ref(false);
const loadError = ref<string | null>(null);

// 顶部「最新」区块：仅展示本模块装卸控制的版本。
function latestCards(): Array<{
  title: string;
  version: GameVersionView | null;
}> {
  return [
    {
      title: t(`${MB_KEY}.latest_release`),
      version: gd.manifest.value?.latest_release ?? null,
    },
    {
      title: t(`${MB_KEY}.latest_preview`),
      version: gd.manifest.value?.latest_preview ?? null,
    },
  ];
}

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
</script>

<template>
  <div class="gd-page">
    <header class="gd-page__header">
      <div class="gd-page__heading">
        <h1 class="gd-page__title">{{ t(`${MB_KEY}.listTitle`) }}</h1>
        <button
          class="gd-page__refresh"
          :title="t(`${MB_KEY}.actions.refresh`)"
          :disabled="localLoading || gd.loading.value"
          @click="refresh"
        >
          <RefreshCw :size="15" :class="{ spin: localLoading || gd.loading.value }" />
        </button>
      </div>
    </header>

    <!-- 骨架 -->
    <div v-if="gd.loading.value && !gd.manifest.value" class="gd-page__scope">
      <div class="gd-latest">
        <div v-for="n in 2" :key="n" class="gd-card gd-card--skeleton">
          <div class="gd-skeleton-line" />
          <div class="gd-skeleton-line gd-skeleton-line--short" />
        </div>
      </div>
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
    <div v-else-if="!gd.manifest.value || gd.manifest.value.groups.length === 0" class="gd-page__state">
      <p class="gd-page__state-text">{{ t(`${MB_KEY}.empty`) }}</p>
      <button class="gd-page__retry" @click="refresh">{{ t(`${MB_KEY}.actions.refresh`) }}</button>
    </div>

    <!-- 内容 -->
    <div v-else class="gd-page__scope">
      <!-- 最新区块 -->
      <div class="gd-latest">
        <div v-for="card in latestCards()" :key="card.title" class="gd-card gd-card--latest">
          <div class="gd-card__title">
            <Rocket :size="15" />
            {{ card.title }}
          </div>
          <VersionRow
            v-if="card.version"
            :version="card.version"
            featured
          />
          <p v-else class="gd-card__empty">{{ t(`${MB_KEY}.empty`) }}</p>
        </div>
      </div>

      <!-- 分组列表 -->
      <section class="gd-groups">
        <h2 class="gd-groups__heading">
          <Layers :size="15" />
          {{ t(`${MB_KEY}.categories`) }}
        </h2>

        <div
          v-for="group in gd.manifest.value.groups"
          :key="group.major"
          class="gd-group"
        >
          <div class="gd-group__head">
            <span class="gd-group__major">Minecraft {{ group.major }}</span>
            <span class="gd-group__count">{{ t(`${MB_KEY}.versionsCount`, { n: group.items.length }) }}</span>
          </div>
          <div class="gd-group__body">
            <VersionRow
              v-for="version in group.items"
              :key="version.id"
              :version="version"
            />
          </div>
        </div>
      </section>
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

.gd-page__header {
  margin-bottom: var(--copper-space-4);
}

.gd-page__heading {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.gd-page__title {
  font-size: var(--copper-font-size-xl);
  font-weight: 700;
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

.gd-latest {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
  gap: var(--copper-space-3);
}

.gd-card {
  padding: var(--copper-space-3);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
}

.gd-card__title {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  margin-bottom: var(--copper-space-2);
}

.gd-card__empty {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.gd-card--skeleton .gd-skeleton-line {
  height: 14px;
  margin-top: var(--copper-space-2);
  border-radius: var(--copper-radius-xs);
  background: var(--copper-surface-3);
}

.gd-card--skeleton .gd-skeleton-line--short {
  width: 60%;
}

.gd-groups {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
}

.gd-groups__heading {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--copper-font-size-lg);
  font-weight: 700;
}

.gd-group {
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  overflow: hidden;
}

.gd-group__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--copper-space-2) var(--copper-space-3);
  border-bottom: 1px solid var(--copper-border);
  background: var(--copper-surface-2);
}

.gd-group__major {
  font-size: var(--copper-font-size-sm);
  font-weight: 700;
}

.gd-group__count {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-secondary);
}

.gd-group__body {
  display: flex;
  flex-direction: column;
  padding: var(--copper-space-1) 0;
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