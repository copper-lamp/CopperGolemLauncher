<script setup lang="ts">
// 版本卡片：列表页与「最新版本」区块共用的最小展示单元。
//
// 版式由需求钉死：
// - 左右**顶满**容器，卡片之间上下排列（不并排）；
// - 大尺寸：卡片本体约为原版的两倍（内边距、图标、字号都放大一档），保证在整屏
//   列表里一眼能扫到版本号，而不是一屏挤下几十行；
// - 左侧内容并排成一行：图标、版本号、徽标（徽标紧跟版本号，在其右边），右侧整块留空；
// - 徽标依次是：版本类型、LeviLamina、本地已有整包（`downloaded`）；
// - 无描边，悬停**只浮起阴影**（不改底色——卡片底色已经和外层白底卡片同色，
//   再叠一层底色变化在浅色主题下会读成「变灰」而不是「高亮」），点击进入二级页面。

import { computed } from "vue";
import { useRouter } from "vue-router";
import { BadgeCheck, Blocks, FlaskConical, Gamepad2, HardDriveDownload } from "@lucide/vue";

import CoBadge from "../../components/ui/CoBadge.vue";
import { useI18n } from "../../i18n";
import type { GameVersionView } from "./api";

const props = defineProps<{ version: GameVersionView }>();

const { t } = useI18n();
const router = useRouter();

const MB_KEY = "module.game-download";

/**
 * 类型徽标的图标：正式版 = 已认证的正式发布，测试版 = 实验品。
 * 图标给了形状记忆点，一排版本号扫过去不必逐字读徽标文字。
 */
const KIND_ICON = { release: BadgeCheck, preview: FlaskConical } as const;
const kindIcon = computed(() => KIND_ICON[props.version.kind]);

function open() {
  void router.push(`/game-download/${encodeURIComponent(props.version.id)}`);
}
</script>

<template>
  <button class="version-card" :title="version.game_version" @click="open">
    <span class="version-card__icon" aria-hidden="true">
      <Gamepad2 :size="20" />
    </span>
    <span class="version-card__name">{{ version.game_version }}</span>
    <CoBadge size="sm" :tone="version.kind" :icon="kindIcon">
      {{ t(`${MB_KEY}.kind.${version.kind}`) }}
    </CoBadge>
    <CoBadge v-if="version.has_loader" size="sm" tone="loader" :icon="Blocks">
      LeviLamina
    </CoBadge>
    <CoBadge v-if="version.downloaded" size="sm" tone="downloaded" :icon="HardDriveDownload">
      {{ t(`${MB_KEY}.downloaded`) }}
    </CoBadge>
  </button>
</template>

<style scoped>
.version-card {
  display: flex;
  align-items: center;
  /* 与徽标之间留一点呼吸，版本号与徽标贴太近会读成一行乱码。 */
  gap: var(--copper-space-2);
  /* 顶满容器：列表里每张卡都占满一行，卡与卡上下排列。 */
  width: 100%;
  min-width: 0;
  padding: var(--copper-space-3);
  /* 无描边：保留透明边框占位，悬停出现阴影时卡片不会因边框出现而位移。 */
  border: 1px solid transparent;
  border-radius: var(--copper-radius-md);
  background: transparent;
  color: var(--copper-text);
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

/* 悬停：只浮起阴影。底色已经是外层卡片的 `--copper-surface`，改成 surface-2 在
 * 浅色主题下就是「整行发灰」，不是高亮。 */
.version-card:hover {
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow);
}

.version-card:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--copper-accent) 45%, transparent);
  outline-offset: -1px;
}

.version-card__icon {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  flex-shrink: 0;
  border-radius: var(--copper-radius-sm);
  background: var(--copper-surface-2);
  color: var(--copper-accent);
}

/* 版本号：卡片里的视觉重点。不参与伸展（flex:0 1 auto），徽标才会紧贴它右侧，
 * 整行内容靠左、右侧整块留白。 */
.version-card__name {
  flex: 0 1 auto;
  min-width: 0;
  margin-left: var(--copper-space-1);
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
  letter-spacing: 0.2px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
