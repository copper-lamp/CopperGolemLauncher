// 内容徽标的「色调 + 文案」映射。
//
// 徽标分三个维度（类型 / 来源 / 发布渠道），颜色统一由 `CoBadge` 的
// `--copper-badge-<tone>` 令牌提供（见 styles/tokens.css），本模块只负责选色调与
// 取文案；视觉实现全在 CoBadge，本文件不含任何样式。
//
// 抽到独立文件的原因：这些函数需被多个组件复用，而 `<script setup>` 不允许导出。

import type { BadgeTone } from "../../components/ui/CoBadge.vue";
import { t } from "../../i18n";

import type { ContentType } from "./api";

const MB_KEY = "module.content-download";

export type { BadgeTone };

/** 内容类型 → 色调（未知类型归入行为包色系，避免无样式）。 */
export function typeBadgeTone(ct: ContentType | string): BadgeTone {
  switch (ct) {
    case "texture_pack":
      return "texture-pack";
    case "shader":
      return "shader";
    case "ll_mod":
      return "ll-mod";
    default:
      return "behavior-pack";
  }
}

/**
 * 内容类型文案（格式标签，如 mcaddon）；无对应文案返回空串，表示不渲染。
 *
 * `lla`（安卓原生模组）虽复用 `ll_mod` 类型，但其分发物是 `.so` 而非
 * BDS 插件包，文案必须区分，否则用户会以为是 LeviLauncher 插件。
 */
export function typeBadgeLabel(
  ct: ContentType | string,
  source?: string,
): string {
  const key = `${MB_KEY}.badge.${source === "lla" ? "lla_mod" : ct}`;
  const label = t(key);
  return label === key ? "" : label;
}

/**
 * 来源 → 色调。
 *
 * 三个来源各自独立取色：CurseForge 灰、LIP 橙、安卓 LL 模组青，
 * 避免用户分不清「桌面 LL 模组」与「安卓原生模组」这两个不同的东西。
 */
export function sourceBadgeTone(src: string): BadgeTone {
  switch (src) {
    case "lip":
      return "source-lip";
    case "lla":
      return "source-lla";
    default:
      return "source-curseforge";
  }
}

/** 来源文案。缺 i18n 键时回落到来源标识本身，不留空徽标。 */
export function sourceBadgeLabel(src: string): string {
  const key = `${MB_KEY}.badge.source_${src === "lla" ? "lla" : src === "lip" ? "lip" : "curseforge"}`;
  const label = t(key);
  return label === key ? src.toUpperCase() : label;
}

/** 发布渠道 → 色调（release / beta / alpha，未知值归入正式版）。 */
export function releaseBadgeTone(rt: string): BadgeTone {
  if (rt === "beta") return "beta";
  if (rt === "alpha") return "alpha";
  return "release";
}

/** 发布渠道文案。 */
export function releaseBadgeLabel(rt: string): string {
  const tone = releaseBadgeTone(rt);
  const key = `${MB_KEY}.badge.${tone}`;
  const label = t(key);
  return label === key ? tone : label;
}
