// 内核提示（tips）API。
//
// 提示是**内核能力**：文案存放在内核语言包的 `tips.items.<序号>` 命名空间下，
// 后端只负责挑选 key，前端拿到 key 后用 `t(key)` 渲染 —— 这样用户切换语言时，
// 正在显示的提示会立刻跟着换语言，无需重新请求后端。

import { call } from "./core";

/** 一条提示：`key` 用于 i18n 渲染，`text` 是后端按当前语言解析好的文案。 */
export interface TipView {
  key: string;
  text: string;
}

/** 取一条随机提示；当前语言下无可用提示（语言包缺失）时返回 `null`。 */
export function tipsNext(): Promise<TipView | null> {
  return call<TipView | null>("tips_next");
}

/** 当前语言下全部提示 key（按序号排序）。 */
export function tipsKeys(): Promise<string[]> {
  return call<string[]>("tips_keys");
}
