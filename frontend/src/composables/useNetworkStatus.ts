import { onMounted, onUnmounted, ref } from "vue";

/** OS/WebView connectivity hint only; does not claim Internet or Xbox presence. */
export function useNetworkStatus() {
  const connected = ref<boolean | null>(typeof navigator !== "undefined" && typeof navigator.onLine === "boolean" ? navigator.onLine : null);
  const update = () => { connected.value = typeof navigator.onLine === "boolean" ? navigator.onLine : null; };
  onMounted(() => { window.addEventListener("online", update); window.addEventListener("offline", update); update(); });
  onUnmounted(() => { window.removeEventListener("online", update); window.removeEventListener("offline", update); });
  return { connected };
}
