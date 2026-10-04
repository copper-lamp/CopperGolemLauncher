// 更新界面的开发期预览（**仅开发构建可见**，入口见 `views/settings/AboutTab.vue`）。
//
// 为什么需要它：更新按钮与弹窗的真实数据源是内核 IPC 的 GitHub 检查，而仓库
// 目前还没有任何 Release，因此在真机上永远看不到「有更新」这条路径长什么样。
// 与其把预览代码塞进组件，不如在这里提供一份与后端同构的 mock 状态，
// 用真实的组件渲染出来确认效果。
//
// 约束（不要越过）：
// - 只在 `import.meta.env.DEV` 下暴露入口，生产构建里按钮不渲染；
// - 只经 `injectPreviewState` 写入状态，不新增任何生产路径分支；
// - 后端一旦发布了真实 Release，这条预览即可删除。

import { injectPreviewState } from "./composables/useUpdate";
import type { UpdateInfo, UpdatePhase, UpdateStatus } from "./api/updater";

const CURRENT = "0.1.0";
const TOTAL = 47_352_320;
const NOW = Math.floor(Date.now() / 1000);

const LATEST: UpdateInfo = {
  version: "0.2.0",
  tag: "v0.2.0",
  notes: [
    "本次更新主要内容",
    "",
    "- 新增：设置页支持自定义下载并发",
    "- 修复：内容下载在代理环境下偶发的校验失败",
    "- 改进：开始页版本列表滚动更顺滑",
  ].join("\n"),
  published_at: "2026-10-01T10:00:00Z",
  html_url: "https://github.com/copper-lamp/CopperGolemLauncher/releases/tag/v0.2.0",
  asset_name: "CopperGolemLauncher-0.2.0-windows-x86_64.zip",
  asset_size: TOTAL,
  download_url: "https://example.invalid/pkg.zip",
  kind: "portable",
  sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
};

function base(phase: UpdatePhase): UpdateStatus {
  return {
    phase,
    current_version: CURRENT,
    latest: LATEST,
    download_task_id: 41,
    error: null,
    error_kind: null,
    active: phase === "downloading" || phase === "downloaded",
    last_checked_at: NOW - 92,
  };
}

/** 依次预览的相位。最后一项回到「已是最新」收尾。 */
const CYCLE: Array<{ labelKey: string; apply: () => void }> = [
  {
    labelKey: "update.badge.available",
    apply: () =>
      injectPreviewState({
        status: { ...base("available"), active: false },
        progress: { ratio: 0.02, downloaded: 947_046, total: TOTAL, speed: 0, etaSeconds: null },
      }),
  },
  {
    labelKey: "update.badge.downloading",
    apply: () =>
      injectPreviewState({
        status: base("downloading"),
        progress: {
          ratio: 0.42,
          downloaded: 19_887_974,
          total: TOTAL,
          speed: 4_194_304,
          etaSeconds: 7,
        },
      }),
  },
  {
    labelKey: "update.badge.downloading",
    apply: () =>
      injectPreviewState({
        status: base("downloading"),
        progress: { ratio: null, downloaded: 8_912_896, total: 0, speed: 1_572_864, etaSeconds: null },
      }),
  },
  {
    labelKey: "update.badge.ready_short",
    apply: () =>
      injectPreviewState({
        status: base("downloaded"),
        progress: { ratio: 1, downloaded: TOTAL, total: TOTAL, speed: 0, etaSeconds: 0 },
      }),
  },
  {
    labelKey: "update.badge.failed_short",
    // 网络类失败时后端查不到版本信息，`latest` 必然为 null。
    apply: () =>
      injectPreviewState({
        status: {
          phase: "failed",
          current_version: CURRENT,
          latest: null,
          download_task_id: null,
          error: "GitHub 接口受限（HTTP 403）：未认证请求每小时仅 60 次，请稍后再试",
          error_kind: "network",
          active: false,
          last_checked_at: NOW - 92,
        },
        progress: null,
      }),
  },
  {
    labelKey: "update.idle_title",
    apply: () =>
      injectPreviewState({
        status: {
          phase: "idle",
          current_version: CURRENT,
          latest: null,
          download_task_id: null,
          error: null,
          error_kind: null,
          active: false,
          last_checked_at: NOW - 92,
        },
        progress: null,
      }),
  },
];

let cursor = -1;

/** 预览用的相位名（给设置页按钮当下一次点击的预告）。 */
export function nextPreviewLabelKey(): string {
  return CYCLE[(cursor + 1) % CYCLE.length]!.labelKey;
}

/**
 * 切到下一个预览相位并打开弹窗。
 *
 * 反复点同一个按钮即可把所有形态过一遍——比在页面上铺一排 mock 开关
 * 更接近「真实界面里只有一个入口」的样子。
 */
export function previewNextUpdateState(): void {
  cursor = (cursor + 1) % CYCLE.length;
  CYCLE[cursor]!.apply();
  injectPreviewState({ panelOpen: true });
}

/** 退出预览，回到「无更新」的真实状态。 */
export function clearPreviewUpdateState(): void {
  cursor = -1;
  injectPreviewState({ panelOpen: false });
}