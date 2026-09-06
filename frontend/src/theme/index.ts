// 主题运行时：把主题服务的快照 / 变更应用到 document。
//
// - mode：dark / light 直接写入 html[data-theme]；auto 跟随系统偏好。
// - accent：#RRGGBB 写入 --copper-accent，并计算可读前景色（黑白极值）。
// - 订阅 `settings.changed`，后端主题变更（含其它窗口 / 模块）实时生效。

import { onSettingsChanged } from "../events";
import { themeSnapshot, type ThemeMode } from "../api/theme";

const root = document.documentElement;

/** 计算强调色上的可读前景（黑 / 白）。 */
function readableForeground(hex: string): string {
  const value = hex.replace("#", "");
  const r = parseInt(value.slice(0, 2), 16);
  const g = parseInt(value.slice(2, 4), 16);
  const b = parseInt(value.slice(4, 6), 16);
  const luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
  return luminance > 140 ? "#0f1419" : "#ffffff";
}

const darkMedia = window.matchMedia("(prefers-color-scheme: dark)");

function applyMode(mode: ThemeMode) {
  const resolved =
    mode === "auto" ? (darkMedia.matches ? "dark" : "light") : mode;
  root.dataset.theme = resolved;
}

function applyAccent(accent: string) {
  const normalized = accent.startsWith("#") ? accent : `#${accent}`;
  root.style.setProperty("--copper-accent", normalized);
  root.style.setProperty("--copper-accent-foreground", readableForeground(normalized));
}

/** 应用一份主题快照。 */
export function applyTheme(mode: ThemeMode, accent: string) {
  applyMode(mode);
  applyAccent(accent);
}

/** 初始化：拉取主题快照并监听设置变更与系统偏好。 */
export async function initTheme(): Promise<void> {
  try {
    const snap = await themeSnapshot();
    applyTheme(snap.mode, snap.accent);
  } catch {
    // 内核未就绪时保持默认令牌。
  }

  darkMedia.addEventListener("change", () => {
    // 仅 auto 模式跟随系统切换。
    const mode = root.dataset.themeMode as ThemeMode | undefined;
    if (mode === "auto") applyMode("auto");
  });

  await onSettingsChanged((changed) => {
    if ("theme.mode" in changed) {
      const mode = changed["theme.mode"];
      if (mode === "dark" || mode === "light" || mode === "auto") {
        root.dataset.themeMode = mode;
        applyMode(mode);
      }
    }
    if ("theme.accent" in changed) {
      const accent = changed["theme.accent"];
      if (typeof accent === "string") applyAccent(accent);
    }
  });
}
