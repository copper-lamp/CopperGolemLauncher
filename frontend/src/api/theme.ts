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

/** 内核信息聚合（版本 / 路径 / 主题 / 语言）。 */
export interface KernelInfo {
  name: string;
  version: string;
  paths: Record<string, string>;
  theme: ThemeSnapshot;
  locales: string[];
}

/** 内核信息聚合。 */
export function kernelInfo(): Promise<KernelInfo> {
  return call<KernelInfo>("kernel_info");
}
