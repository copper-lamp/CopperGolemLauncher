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
    for (const [name, fallback] of Object.entries(durations)) {
      const key = name.replace(/-/g, "_");
      const value = bounded(get(`appearance.motion.${key}`, fallback), fallback, 2000);
      root.style.setProperty(`--copper-motion-${name}`, `${motionEnabled.value ? value : 0}ms`);
    }
  });
}
