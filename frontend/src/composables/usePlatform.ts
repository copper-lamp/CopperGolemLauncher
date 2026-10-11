// 平台形态：桌面 / 移动两态布局的唯一依据。
//
// 权威来源是内核 `kernel_info` 的 `platform` / `formFactor`（见
// docs/平台适配.md 2.5）：内核按编译目标 `cfg` 判定，比 UA 嗅探可靠。
// `navigator.userAgent` 只在**内核未就绪**（浏览器调试 / 后端异常）时兜底，
// 保证首屏不会因平台未知而选错布局取向。

import { computed, ref } from "vue";

import { kernelInfo } from "../api/theme";

/** 平台枚举值（与内核 / 注册表 `platforms` 字段逐字一致）。 */
export type PlatformId =
  | "windows-x86_64"
  | "windows-aarch64"
  | "android-arm64"
  | "linux-x86_64";

/** 平台形态：桌面（窗口 + 左导航）/ 移动（全屏 + 底部标签栏）。 */
export type FormFactor = "desktop" | "mobile";

/** 屏幕取向：决定移动端采用左导航（横屏）还是底部标签栏（竖屏）。 */
export type Orientation = "portrait" | "landscape";

const platform = ref<PlatformId>("windows-x86_64");
const formFactor = ref<FormFactor>("desktop");
const orientation = ref<Orientation>("landscape");

/**
 * 内核未就绪时的兜底探测。
 *
 * 移动端 UA 含 `Android`；Linux 需排除 Android（桌面 Linux 的 UA 也含 Linux）。
 * 架构仅区分 arm64 与 x64，与本项目「单一工具链」的现状一致。
 */
function detectPlatformFallback(): PlatformId {
  if (typeof navigator === "undefined") return "windows-x86_64";
  const ua = navigator.userAgent;
  if (/android/i.test(ua)) return "android-arm64";
  if (/aarch64|arm64/i.test(ua)) return "windows-aarch64";
  if (/linux/i.test(ua)) return "linux-x86_64";
  return "windows-x86_64";
}

/** 由平台枚举推导形态（内核未给 `formFactor` 时的兜底）。 */
function formFactorOf(id: PlatformId): FormFactor {
  return id === "android-arm64" ? "mobile" : "desktop";
}

/** 把形态写到 `<html data-form-factor>`，供 CSS 选择器（安全区 / 导航取向）使用。 */
function applyFormFactor(value: FormFactor): void {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.formFactor = value;
}

/**
 * 探测屏幕取向。
 *
 * 优先用 `matchMedia("(orientation: landscape)")`（与 CSS 媒体特性同源，避免读写
 * 宽高的时序差异）；无 `matchMedia` 时退回宽高比较。移动端横屏要换成左导航，
 * 所以取向必须与 CSS 判定一致，不能各算各的。
 */
function detectOrientation(): Orientation {
  if (typeof window === "undefined") return "landscape";
  if (typeof window.matchMedia === "function") {
    return window.matchMedia("(orientation: landscape)").matches ? "landscape" : "portrait";
  }
  return window.innerWidth >= window.innerHeight ? "landscape" : "portrait";
}

/** 把取向写到 `<html data-orientation>`，供 CSS 选择器使用。 */
function applyOrientation(value: Orientation): void {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.orientation = value;
}

/** 注册取向变化监听：旋转屏幕时 `orientation` 需实时跟随。 */
function watchOrientation(): void {
  if (typeof window === "undefined") return;
  const update = () => {
    const next = detectOrientation();
    if (next === orientation.value) return;
    orientation.value = next;
    applyOrientation(next);
  };
  window.addEventListener("resize", update);
  window.addEventListener("orientationchange", update);
}

/**
 * 同步读取当前平台枚举值。
 *
 * 供非响应式场景使用（如模块注册表的兼容性判定）。启动时
 * [`initPlatform`] 一旦拿到内核权威值，此处即随之更新。
 */
export function currentPlatform(): PlatformId {
  return platform.value;
}

/** 平台形态组合式（响应式）。 */
export function usePlatform() {
  return {
    platform: computed(() => platform.value),
    formFactor: computed(() => formFactor.value),
    isMobile: computed(() => formFactor.value === "mobile"),
    /** 屏幕取向（响应式）：横屏移动端改用左导航，与桌面一致。 */
    orientation: computed(() => orientation.value),
    isLandscape: computed(() => orientation.value === "landscape"),
    /**
     * 是否 Android。与 `isMobile` 分开：移动形态还包含将来的 iOS，
     * 而「LL 模组走 `.so` 直装」是 Android 专属能力（见
     * docs/模块/内容下载/设计.md 安卓章节）。
     */
    isAndroid: computed(() => platform.value === "android-arm64"),
  };
}

/**
 * 初始化平台形态。
 *
 * 容错策略与 `markKernelReady` 一致：**失败不抛出**。平台信息只影响布局取向，
 * 拿不到就退回 UA 兜底，绝不因此阻断首屏渲染。
 */
export async function initPlatform(): Promise<void> {
  // 取向必须在任何 await 之前就定下来并挂上监听：横屏移动端要用左导航，
  // 若等 IPC 超时后才判定，首帧会先按竖屏布局闪一下。
  orientation.value = detectOrientation();
  applyOrientation(orientation.value);
  watchOrientation();

  let id = detectPlatformFallback();
  let form = formFactorOf(id);
  try {
    const info = await kernelInfo();
    if (info.platform) id = info.platform as PlatformId;
    form =
      info.formFactor === "desktop" || info.formFactor === "mobile"
        ? info.formFactor
        : formFactorOf(id);
  } catch {
    // 内核不可用：沿用兜底值。
  }
  platform.value = id;
  formFactor.value = form;
  applyFormFactor(form);
}
