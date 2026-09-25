// 主题 API。

import { call } from "./core";

export type ThemeMode = "dark" | "light" | "auto";

/** 主题快照（前端启动时一次性应用）。 */
export interface ThemeSnapshot {
  mode: ThemeMode;
  accent: string;
}

/** 主题快照。 */
export function themeSnapshot(): Promise<ThemeSnapshot> {
  return call<ThemeSnapshot>("theme_snapshot");
}

/** 设置深浅色模式。 */
export function themeSetMode(mode: ThemeMode): Promise<void> {
  return call<void>("theme_set_mode", { mode });
}

/** 设置强调色（#RRGGBB）。 */
export function themeSetAccent(hex: string): Promise<void> {
  return call<void>("theme_set_accent", { hex });
}

/** 内核信息聚合（版本 / 平台 / 路径 / 主题 / 语言）。 */
export interface KernelInfo {
  name: string;
  version: string;
  /**
   * 平台枚举值（如 `windows-x86_64` / `android-arm64`）。
   *
   * 声明为可选：容忍内核尚未下发该字段的旧版本，此时前端退回 UA 兜底
   * （见 `composables/usePlatform.ts`）。
   */
  platform?: string;
  /** 平台形态：驱动 Shell 桌面 / 移动两态布局。 */
  formFactor?: "desktop" | "mobile";
  paths: Record<string, string>;
  theme: ThemeSnapshot;
  locales: string[];
}

/** 内核信息聚合。 */
export function kernelInfo(): Promise<KernelInfo> {
  return call<KernelInfo>("kernel_info");
}
