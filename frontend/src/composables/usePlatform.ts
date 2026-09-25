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

const platform = ref<PlatformId>("windows-x86_64");
const formFactor = ref<FormFactor>("desktop");

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
  };
}

/**
 * 初始化平台形态。
 *
 * 容错策略与 `markKernelReady` 一致：**失败不抛出**。平台信息只影响布局取向，
 * 拿不到就退回 UA 兜底，绝不因此阻断首屏渲染。
 */
export async function initPlatform(): Promise<void> {
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
