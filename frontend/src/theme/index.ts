// 主题运行时：响应式状态 + 立即应用 + 内核同步。
//
// - `themeState` 为响应式状态（mode / accent），设置页与其它视图共用。
// - 写入（setThemeMode / setAccent）**先本地立即应用**，再异步写回内核；
//   事件（`settings-changed`）仅作为外部来源（其它窗口 / 模块 / 后端）的校准。
// - mode：dark / light 写入 html[data-theme]；time 按本地时间；auto 跟随系统偏好。
// - accent：#RRGGBB 写入 --copper-accent，并计算可读前景色。

import { reactive } from "vue";
import { initVisualPreferences } from "../composables/useVisualPreferences";

initVisualPreferences();

import { onSettingsChanged } from "../events";
import { themeSnapshot, themeSetMode, themeSetAccent, type ThemeMode } from "../api/theme";

export type { ThemeMode } from "../api/theme";

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
let timeTimer: number | undefined;
function timeTheme(): "dark" | "light" {
  const now = new Date();
  const minutes = now.getHours() * 60 + now.getMinutes();
  return minutes >= 7 * 60 && minutes < 20 * 60 ? "light" : "dark";
}
/** 下一个 07:00 / 20:00 边界（本地时间）。 */
function nextTimeBoundary(): number {
  const now = new Date();
  const minutes = now.getHours() * 60 + now.getMinutes();
  const next = new Date(now);
  if (minutes < 7 * 60) {
    next.setHours(7, 0, 0, 0);
  } else if (minutes < 20 * 60) {
    next.setHours(20, 0, 0, 0);
  } else {
    next.setDate(next.getDate() + 1);
    next.setHours(7, 0, 0, 0);
  }
  return next.getTime();
}

function scheduleTimeTheme(): void {
  if (timeTimer !== undefined) window.clearTimeout(timeTimer);
  timeTimer = undefined;
  if (themeState.mode !== "time") return;
  const delay = Math.max(1000, nextTimeBoundary() - Date.now());
  timeTimer = window.setTimeout(() => {
    applyMode("time");
    scheduleTimeTheme();
  }, delay);
}

/** 响应式主题状态：默认与内核默认一致，启动时被快照覆盖。 */
export const themeState = reactive<{ mode: ThemeMode; accent: string }>({
  mode: "dark",
  accent: "#c97b3d",
});

function applyMode(mode: ThemeMode) {
  const resolved =
    mode === "auto" ? (darkMedia.matches ? "dark" : "light") : mode === "time" ? timeTheme() : mode;
  root.dataset.theme = resolved;
  root.dataset.themeMode = mode;
  scheduleTimeTheme();
}

function applyAccent(accent: string) {
  const normalized = accent.startsWith("#") ? accent : `#${accent}`;
  root.style.setProperty("--copper-accent", normalized);
  root.style.setProperty(
    "--copper-accent-foreground",
    readableForeground(normalized),
  );
}

/** 把当前状态渲染到 document。 */
function render() {
  applyMode(themeState.mode);
  applyAccent(themeState.accent);
}

/** 初始化：拉取快照、应用主题，并监听系统偏好与外部变更。 */
export async function initTheme(): Promise<void> {
  try {
    const snap = await themeSnapshot();
    themeState.mode = snap.mode;
    themeState.accent = snap.accent;
  } catch {
    // 内核未就绪时保持默认令牌。
  }
  render();

  darkMedia.addEventListener("change", () => {
    // 仅 auto 模式跟随系统切换。
    if (themeState.mode === "auto") applyMode("auto");
  });

  // 系统休眠/唤醒后计时器可能晚到：回到前台时按当前时间重新判定。
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && themeState.mode === "time") applyMode("time");
  });

  await onSettingsChanged((changed) => {
    let dirty = false;
    if ("theme.mode" in changed) {
      const mode = changed["theme.mode"];
      if (mode === "dark" || mode === "light" || mode === "time" || mode === "auto") {
        themeState.mode = mode;
        dirty = true;
      }
    }
    if ("theme.accent" in changed) {
      const accent = changed["theme.accent"];
      if (typeof accent === "string") {
        themeState.accent = accent;
        dirty = true;
      }
    }
    if (dirty) render();
  });
}

/** 设置主题模式：本地立即生效，再异步写回内核（事件兜底校准）。 */
export async function setThemeMode(mode: ThemeMode): Promise<void> {
  themeState.mode = mode;
  render();
  try {
    await themeSetMode(mode);
  } catch {
    // 写回失败不影响本地生效；内核就绪后由事件校准。
  }
}

/** 设置强调色：本地立即生效，再异步写回内核。 */
export async function setAccent(hex: string): Promise<void> {
  themeState.accent = hex;
  render();
  try {
    await themeSetAccent(hex);
  } catch {
    // 同上。
  }
}
