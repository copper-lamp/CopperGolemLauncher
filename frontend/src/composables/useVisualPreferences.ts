// Visual preferences consume the existing settings snapshot; no IPC blocks rendering.
import { computed, ref, watchEffect } from "vue";
import { useSettings } from "./useSettings";

const reduced = ref(window.matchMedia("(prefers-reduced-motion: reduce)").matches);
const { get } = useSettings();
export const motionEnabled = computed(() => get<boolean>("appearance.animations", true) !== false && !reduced.value);
let initialized = false;
const durations = {
  "component-enter": 180, "component-exit": 120, "component-press": 80,
  "page-enter": 220, "page-exit": 140, "loading-enter": 160, "loading-exit": 120,
} as const;

/** 动画预设：以「流畅」为 1 倍基准。快速更快，丝滑更缓（见设置 → 个性化）。 */
const motionPresets: Record<string, number> = { fast: 0.7, smooth: 1, silky: 1.4 };
function bounded(value: unknown, fallback: number, max: number): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(0, Math.min(max, value)) : fallback;
}
export function initVisualPreferences(): void {
  if (initialized) return;
  initialized = true;
  const media = window.matchMedia("(prefers-reduced-motion: reduce)");
  media.addEventListener("change", () => { reduced.value = media.matches; });
  watchEffect(() => {
    const root = document.documentElement;
    root.dataset.motion = motionEnabled.value ? "full" : "reduced";
    root.style.setProperty("--copper-radius-card", `${bounded(get("appearance.radius", 16), 16, 32)}px`);
    root.style.setProperty("--copper-radius-control", "var(--copper-radius-card)");
    const preset = motionPresets[get<string>("appearance.motion_preset", "smooth")] ?? 1;
    for (const [name, fallback] of Object.entries(durations)) {
      const key = name.replace(/-/g, "_");
      const base = bounded(get(`appearance.motion.${key}`, fallback), fallback, 2000);
      const value = Math.round(Math.min(2000, base * preset));
      root.style.setProperty(`--copper-motion-${name}`, `${motionEnabled.value ? value : 0}ms`);
    }
  });
}
