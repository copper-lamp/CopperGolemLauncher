<script setup lang="ts">
// 内核首页：内核阶段展示内核就绪信息；模块开发阶段由"开始页"模块接管此路由。

import { onMounted, ref } from "vue";

import { kernelInfo, type KernelInfo } from "../api/theme";
import { useI18n } from "../i18n";

const { t } = useI18n();

const info = ref<KernelInfo | null>(null);
const paths = ref<Array<[string, string]>>([]);

onMounted(async () => {
  try {
    info.value = await kernelInfo();
    paths.value = Object.entries(info.value.paths);
  } catch {
    // 内核未就绪时保持占位。
  }
});
</script>

<template>
  <div class="home">
    <div class="home__hero">
      <div class="home__logo">
        <svg viewBox="0 0 24 24" width="56" height="56" aria-hidden="true">
          <path
            d="M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Zm3 8h8v2H8v-2Zm0 4h5v2H8v-2Zm0-8h8v2H8V7Z"
            fill="currentColor"
          />
        </svg>
      </div>
      <h1 class="home__title">{{ t("app.name") }}</h1>
      <p class="home__tagline">{{ t("app.tagline") }}</p>
      <p class="home__version">
        {{ t("settings.about.version") }}：
        <span class="home__version-value">{{ info?.version ?? "0.1.0" }}</span>
      </p>
    </div>

    <section v-if="info" class="home__paths">
      <h2 class="home__section-title">{{ t("common.open_folder") }}</h2>
      <ul class="home__path-list">
        <li v-for="[key, value] in paths" :key="key" class="home__path-item">
          <span class="home__path-key">{{ key }}</span>
          <span class="home__path-value">{{ value }}</span>
        </li>
      </ul>
    </section>
  </div>
</template>

<style scoped>
.home {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-6);
  padding: var(--copper-space-6);
  overflow-y: auto;
}

.home__hero {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-2);
}

.home__logo {
  width: 96px;
  height: 96px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--copper-radius-lg);
  color: var(--copper-accent);
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
}

.home__title {
  font-size: var(--copper-font-size-xl);
  font-weight: 700;
}

.home__tagline {
  color: var(--copper-text-secondary);
}

.home__version {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.home__version-value {
  color: var(--copper-text);
  font-weight: 600;
}

.home__paths {
  width: min(520px, 100%);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  padding: var(--copper-space-4);
}

.home__section-title {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  margin-bottom: var(--copper-space-3);
}

.home__path-list {
  list-style: none;
}

.home__path-item {
  display: flex;
  justify-content: space-between;
  gap: var(--copper-space-3);
  padding: var(--copper-space-1) 0;
  font-size: var(--copper-font-size-sm);
}

.home__path-key {
  color: var(--copper-text-secondary);
  flex-shrink: 0;
}

.home__path-value {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
}
</style>
