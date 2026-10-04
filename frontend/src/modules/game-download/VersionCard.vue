<script setup lang="ts">
// 版本卡片：列表页与「最新版本」区块共用的最小展示单元。
//
// 版式由需求钉死：**左侧**依次是图标、版本号、徽标，**右侧什么都不显示**。
// 卡片本身无描边，靠悬停阴影与背景变化表达可点；点击进入该版本的二级页面
// （安装入口在那里，列表页只做「看」与「进」）。

import { useRouter } from "vue-router";
import { Gamepad2 } from "@lucide/vue";

import { useI18n } from "../../i18n";
import type { GameVersionView } from "./api";

const props = defineProps<{ version: GameVersionView }>();

const { t } = useI18n();
const router = useRouter();

const MB_KEY = "module.game-download";

function open() {
  void router.push(`/game-download/${encodeURIComponent(props.version.id)}`);
}
</script>

<template>
  <button class="version-card" :title="version.game_version" @click="open">
    <span class="version-card__icon" aria-hidden="true">
      <Gamepad2 :size="18" />
    </span>
    <span class="version-card__body">
      <span class="version-card__name">{{ version.game_version }}</span>
      <span class="version-card__badges">
        <span class="version-card__badge" :class="`version-card__badge--${version.kind}`">
          {{ t(`${MB_KEY}.kind.${version.kind}`) }}
        </span>
        <span v-if="version.has_loader" class="version-card__badge version-card__badge--loader">
          LeviLamina
        </span>
      </span>
    </span>
  </button>
</template>

<style scoped>
.version-card {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  width: 100%;
  min-width: 0;
  padding: var(--copper-space-2) var(--copper-space-3);
  /* 无描边：保留透明边框占位，悬停出现阴影时卡片不会因边框出现而位移。 */
  border: 1px solid transparent;
  border-radius: var(--copper-radius-lg);
  background: transparent;
  color: var(--copper-text);
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.version-card:hover {
  background: var(--copper-surface-2);
  box-shadow: var(--copper-shadow);
  transform: translateY(-1px);
}

.version-card:active {
  transform: translateY(0);
}

.version-card:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--copper-accent) 45%, transparent);
  outline-offset: 1px;
}

.version-card__icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  flex-shrink: 0;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-accent);
}

.version-card__body {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 3px;
  min-width: 0;
}

.version-card__name {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  white-space: nowrap;
}

.version-card__badges {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
  flex-wrap: wrap;
}

.version-card__badge {
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  font-size: var(--copper-font-size-xs);
  line-height: 1.5;
  border: 1px solid transparent;
}

.version-card__badge--release {
  color: var(--copper-badge-release);
  background: var(--copper-badge-release-bg);
  border-color: var(--copper-badge-release-border);
}

.version-card__badge--preview {
  color: var(--copper-badge-alpha);
  background: var(--copper-badge-alpha-bg);
  border-color: var(--copper-badge-alpha-border);
}

.version-card__badge--loader {
  color: var(--copper-badge-ll-mod);
  background: var(--copper-badge-ll-mod-bg);
  border-color: var(--copper-badge-ll-mod-border);
}
</style>
