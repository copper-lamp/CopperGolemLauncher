// 意图 API：发起意图请求、查询已声明意图。

import type { JsonValue } from "./types";
import { call } from "./core";

/** 发起意图请求（请求 / 响应式模块联动）。 */
export function intentsRequest(intent: string, payload: JsonValue): Promise<JsonValue> {
  return call<JsonValue>("intents_request", { intent, payload });
}

/** 已声明意图清单（模块名 -> 意图名）。 */
export function intentsDeclared(): Promise<Array<[string, string]>> {
  return call<Array<[string, string]>>("intents_declared");
}
