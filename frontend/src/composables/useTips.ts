// 提示（tips）状态：全局单例，供加载页等「等待中」界面展示随机提示。
//
// 设计要点：
// - **懒加载**：`initTips()` 不预取提示。提示只在真正要显示的界面存在时才有价值，
//   预取会在每次启动都产生一次无意义的 IPC（后端要读语言包并挑选）。
// - **存 key 不存文案**：组件用 `t(key)` 渲染，语言切换时正在显示的提示即时跟随。
// - **引用计数轮换**：同一时刻可能有多个组件显示提示（如加载页与模块加载态），
//   单独启动的计时器会互相覆盖。用引用计数保证第一个组件启动、最后一个组件停止，
//   只有一个组件时行为同样正确。
// - **不依赖组件生命周期**：`stopRotation()` 幂等且可在任意上下文调用，
//   组件忘记 stop 只会多一次定时轮换，不会泄漏计时器或抛错。

import { readonly, ref } from "vue";

import { tipsNext } from "../api/tips";

/** 轮换时是否已经有提示在展示（供组件做淡入淡出判断）。 */
const rotating = ref(false);

/** 当前提示的 i18n key；为 `null` 表示无提示可显示。 */
const currentTipKey = ref<string | null>(null);

/** 上一次后端返回的文案。仅作诊断/非 i18n 场景备用，渲染一律用 key。 */
const currentTipText = ref<string | null>(null);

/** 启动者数量（引用计数）。为 0 表示没有组件在展示提示。 */
let holders = 0;

/** 轮换计时器句柄。已停表时为 `null`。 */
let timer: ReturnType<typeof setInterval> | null = null;

/** 轮换间隔（首个启动者设定；后续启动者不覆盖，避免互相抢占）。 */
let intervalMs = 6000;

/**
 * 拉取一条新提示并更新全局状态。
 *
 * 失败（内核未就绪 / 命令报错）时**清空**提示而不是保留上一条：
 * 提示是装饰性内容，宁可整条不显示，也不要展示过期或语言不匹配的文案。
 */
export async function rotateTip(): Promise<void> {
  try {
    const tip = await tipsNext();
    currentTipKey.value = tip?.key ?? null;
    currentTipText.value = tip?.text ?? null;
  } catch {
    currentTipKey.value = null;
    currentTipText.value = null;
  }
}

/** 初始化：当前无需预取；保留此入口以便后续接入内核事件（如语言变更后重新挑 key）。 */
export async function initTips(): Promise<void> {
  // 故意留空：提示按需拉取，`rotateTip()` 才是唯一的数据来源。
}

/**
 * 开始轮换。
 *
 * 第一个调用者立即取一条提示（避免用户先看到几百毫秒的空界面），随后按
 * `intervalMs` 定时轮换；重复调用只增加引用计数，不会重复建表。
 *
 * @param ms 轮换间隔；仅首个调用者的值生效。
 */
export function startRotation(ms = 6000): void {
  holders += 1;
  if (timer !== null) return;

  rotating.value = true;
  intervalMs = ms;
  void rotateTip();
  timer = setInterval(() => void rotateTip(), intervalMs);
}

/**
 * 停止轮换（幂等）。
 *
 * 引用计数归零时真正停表；多余调用只是计数降到 0 后不再变化，不会重复生效。
 */
export function stopRotation(): void {
  holders = Math.max(0, holders - 1);
  if (holders > 0) return;

  if (timer !== null) {
    clearInterval(timer);
    timer = null;
  }
  rotating.value = false;
}

/** 可组合式入口（全局单例，所有调用者共享同一份状态）。 */
export function useTips() {
  return {
    /** 当前提示 key；`null` 表示无提示。 */
    currentTipKey: readonly(currentTipKey),
    /** 当前提示文案（后端解析结果，备用）。 */
    currentTipText: readonly(currentTipText),
    /** 是否处于轮换中。 */
    rotating: readonly(rotating),
    initTips,
    rotateTip,
    startRotation,
    stopRotation,
  };
}
