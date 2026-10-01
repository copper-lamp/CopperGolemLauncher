// 应用入口：装配样式、路由、i18n、主题与全局状态。
//
// # 引导原则（安卓卡加载事故的直接结论）
//
// UI 的可见性**不得**依赖内核 IPC：Tauri 的 `invoke` 在 IPC 桥缺失时既不 resolve
// 也不 reject（见 `boot.ts::hasIpcBridge` 的说明），一旦把它放在渲染链路上，
// 表现就是「永远加载中、点哪都没反应」。
//
// 因此引导拆成两段：
// - **必须成功的本地装配**：import 资源 + `app.mount`。这一步不碰 IPC，
//   只要脚本能跑完就一定出画面；
// - **并行、带超时、允许失败的内核能力装配**：每一项都有独立超时与降级，
//   超时只登记到诊断面板，不再阻断界面。
//
// 每一步都登记进 `boot.ts` 的步骤表，出问题时用户可在诊断面板里直接复制报告。

import { createApp, watch } from "vue";

import App from "./App.vue";
import router from "./router";

import "./styles/tokens.css";
import "./styles/base.css";

import { initI18n } from "./i18n";
import { initTheme } from "./theme";
import { initPlatform } from "./composables/usePlatform";
import { initSettings } from "./composables/useSettings";
import { initDownloads } from "./composables/useDownloads";
import { initAccount } from "./composables/useAccount";
import { markKernelReady } from "./composables/useKernelReady";
import { observeAndroidGameExit } from "./composables/useAndroidGameExit";
import { loadAddonFrontends } from "./modules/addonRuntime";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import {
  beginStep,
  endStep,
  failStep,
  forward,
  hasIpcBridge,
  hasTauriInternals,
  installBootDiagnostics,
  logBoot,
  setIpcState,
  timeoutStep,
  useBootDiagnostics,
  waitForIpcBridge,
} from "./boot";
import { hasBootFailure, openDiagnosticOnFailure, setDiagnosticOpen } from "./diag";

/** 内核能力装配的单步超时：超过即降级，不再等（界面已经可见，等待无意义）。 */
const KERNEL_STEP_TIMEOUT_MS = 5000;

/**
 * 等待宿主注入 ipc 桥的期限。
 *
 * 刻意长于单步超时：桥的注入时机受 WebView 能力影响，可能晚于应用脚本，
 * 但一旦确认缺失就是「所有命令都不会有响应」的硬故障，值得多等一会儿再下结论。
 */
const IPC_BRIDGE_TIMEOUT_MS = 10000;

/** 附加模块前端装载超时（与改造前保持一致：模块不该拖慢启动）。 */
const ADDON_BOOTSTRAP_TIMEOUT_MS = 3000;

// 最早安装：引导期异常发生在首帧之前，晚一步就只剩「卡加载」这一个现象。
installBootDiagnostics();

const app = createApp(App);
app.use(router);

// 渲染期异常必须留下痕迹，否则用户看到的只是一个不再刷新的界面。
app.config.errorHandler = (error, _instance, info) => {
  const message = `${String(error)} (${info})`;
  logBoot("error", `vue: ${message}`);
  forward(message);
};

/**
 * 给一个引导步骤叠加超时。
 *
 * 注意区分两种失败：抛出（后端明确报错）与超时（调用没回来）。前者多半可直接
 * 降级使用，后者说明 IPC 链路可疑，需要在诊断面板里高亮。
 */
async function withTimeout(id: string, labelKey: string, task: () => Promise<void>): Promise<void> {
  beginStep(id, labelKey);
  let timer = 0;
  const timeout = new Promise<"timeout">((resolve) => {
    timer = window.setTimeout(() => resolve("timeout"), KERNEL_STEP_TIMEOUT_MS);
  });
  try {
    const outcome = await Promise.race([task().then(() => "ok" as const), timeout]);
    if (outcome === "timeout") {
      timeoutStep(id, KERNEL_STEP_TIMEOUT_MS);
      logBoot("warn", `${id} 超时（${KERNEL_STEP_TIMEOUT_MS}ms），已降级继续`);
    } else {
      endStep(id);
    }
  } catch (error) {
    failStep(id, error);
    logBoot("error", `${id} 失败：${String(error)}`);
  } finally {
    window.clearTimeout(timer);
  }
}

/**
 * 桥存在性判定。
 *
 * 与内核探活分开：桥从未出现说明宿主初始化脚本没跑到应用脚本之前（安卓上
 * wry 的注入方式会随 WebView 特性支持度退化），桥在而命令不回则是内核处理
 * 侧的问题。两者修复方向完全不同，报告里必须能一眼区分。
 */
async function probeBridge(): Promise<void> {
  beginStep("bridge", "boot.error.step.bridge");
  logBoot(
    "info",
    `tauri globals: internals=${hasTauriInternals()} ipc=${hasIpcBridge()}`,
  );
  if (await waitForIpcBridge(IPC_BRIDGE_TIMEOUT_MS)) {
    endStep("bridge");
    setIpcState("ready");
    logBoot("info", "Tauri ipc 桥可用");
    return;
  }
  // 桥始终没出现：所有 invoke 都会静默排队（既不 resolve 也不 reject），
  // 界面必然点不动。这类失败必须点名，否则用户只能看到「一直加载中」。
  timeoutStep("bridge", IPC_BRIDGE_TIMEOUT_MS);
  setIpcState("unavailable");
  logBoot(
    "error",
    `Tauri 初始化脚本未注入 ipc 桥（internals=${hasTauriInternals()}），内核命令将无响应`,
  );
}

/**
 * 判断内核是否真的在应答。
 *
 * 直接用内核命令探活，而不是只看 `window.__TAURI_INTERNALS__.ipc` 是否存在：
 * 桥在、命令不回同样会让界面假死，用户报告里必须能区分这两种情况。
 */
async function probeKernel(): Promise<void> {
  beginStep("kernel", "boot.error.step.kernel");
  logBoot("info", `tauri internals: ${hasTauriInternals()}`);
  try {
    await Promise.race([
      invoke("kernel_info"),
      new Promise((_, reject) =>
        window.setTimeout(
          () => reject(new Error(`kernel_info timeout after ${KERNEL_STEP_TIMEOUT_MS}ms`)),
          KERNEL_STEP_TIMEOUT_MS,
        ),
      ),
    ]);
    endStep("kernel");
    setIpcState("ready");
    logBoot("info", "kernel_info 应答正常");
  } catch (error) {
    // 探活失败不阻断：平台 / 主题 / 语言各自有本地兜底。
    failStep("kernel", error);
    setIpcState("unavailable");
    logBoot("error", `内核探活失败：${String(error)}`);
  }
}

// 1) 先把界面点亮：本地装配 + 挂载不依赖任何 IPC。
// 附加模块前端必须在挂载**之前**登记（左导航与路由表在首次渲染时被读取），
// 因此这里只等一个带超时的模块装配，最多多等 ADDON_BOOTSTRAP_TIMEOUT_MS。
beginStep("addons", "boot.error.step.addons");
await Promise.race([
  loadAddonFrontends(router),
  new Promise<never>((_, reject) =>
    window.setTimeout(
      () => reject(new Error(`addon bootstrap timeout after ${ADDON_BOOTSTRAP_TIMEOUT_MS}ms`)),
      ADDON_BOOTSTRAP_TIMEOUT_MS,
    ),
  ),
])
  .then(() => endStep("addons"))
  .catch((error) => {
    // 单个模块失败不该阻断内置界面（见 addonRuntime 的错误隔离）。
    failStep("addons", error);
    logBoot("warn", `附加模块前端装配失败：${String(error)}`);
    forward(`addon frontend bootstrap failed: ${String(error)}`);
  });

app.mount("#app");
markKernelReady();
logBoot("info", "界面已挂载，开始装配内核能力");

// 2) 内核能力并行装配：任何一项失败 / 超时都只影响它自己的能力。
void Promise.all([
  probeBridge(),
  probeKernel(),
  withTimeout("theme", "boot.error.step.theme", initTheme),
  withTimeout("i18n", "boot.error.step.i18n", initI18n),
  withTimeout("platform", "boot.error.step.platform", initPlatform),
  withTimeout("settings", "boot.error.step.settings", initSettings),
  withTimeout("downloads", "boot.error.step.downloads", initDownloads),
  withTimeout("account", "boot.error.step.account", initAccount),
]).then(() => logBoot("info", "内核能力装配流程结束"));

void listen<{ instance_name: string; package_name: string; version_name: string }>(
  "android-game-prepare",
  async (event) => {
    const instance = encodeURIComponent(event.payload.instance_name);
    const version = encodeURIComponent(event.payload.version_name ?? "");
    await openExternal(`coppergolem://game?instance_name=${instance}&version_name=${version}`).catch(
      (error: unknown) => {
        forward(`android game bridge failed: ${String(error)}`);
      },
    );
  },
);

// 游戏退出不走事件总线：原生库不可卸载，退出只能由 Java 宿主写文件信箱，
// 详见 composables/useAndroidGameExit.ts。
observeAndroidGameExit();

// 3) 诊断出口：引导失败 / 超时自动展开面板；`?diag=1` 供用户主动打开。
// 没有这一步，安卓上「点不动」的现场就只剩用户的一句「加载中」。
if (new URLSearchParams(location.search).get("diag") === "1") {
  setDiagnosticOpen(true);
}
const { steps: bootSteps } = useBootDiagnostics();
watch(
  bootSteps,
  () => {
    if (hasBootFailure()) openDiagnosticOnFailure();
  },
  { immediate: true, deep: true },
);
