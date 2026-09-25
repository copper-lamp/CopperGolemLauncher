// 内容徽标的「色调 + 文案」映射。
//
// 徽标分三个维度，颜色统一由主题令牌 `--copper-badge-<tone>` 提供
// （深色 / 浅色各一套，见 styles/tokens.css），本模块只负责选色调与取文案。
//
// 抽到独立文件的原因：这些函数需被多个组件复用，而 `<script setup>` 不允许导出。

import { t } from "../../i18n";

import type { ContentType } from "./api";

const MB_KEY = "module.content-download";

/** 徽标色调：对应 `--copper-badge-<tone>` 令牌组。 */
export type BadgeTone =
  | "behavior-pack"
  | "texture-pack"
  | "shader"
  | "ll-mod"
  | "source-curseforge"
  | "source-lip"
  | "release"
  | "beta"
  | "alpha";

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

/** 内容类型文案（格式标签，如 mcaddon）；无对应文案返回空串，表示不渲染。 */
export function typeBadgeLabel(ct: ContentType | string): string {
  const key = `${MB_KEY}.badge.${ct}`;
  const label = t(key);
  return label === key ? "" : label;
}

/** 来源 → 色调。 */
export function sourceBadgeTone(src: string): BadgeTone {
  return src === "lip" ? "source-lip" : "source-curseforge";
}

/** 来源文案。 */
export function sourceBadgeLabel(src: string): string {
  const key = `${MB_KEY}.badge.source_${src === "lip" ? "lip" : "curseforge"}`;
  const label = t(key);
  return label === key ? (src === "lip" ? "LIP" : "CurseForge") : label;
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
