// 通用 JSON 值类型（serde_json::Value 的 TS 对应）。

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };
