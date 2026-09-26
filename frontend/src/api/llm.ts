// LLM 模型表 API：列出 / 增删改模型，写入 / 清除单条模型的密钥。
//
// 内核极简——只存用户维护的模型表（base URL / 模型名称 / 可选显示名称），
// 不预设 provider、base URL 或「默认模型」；调用方按 id 指定要用的条目。
// 密钥走系统密钥环且绝不回显：列表只回每条「是否已配置」。

import { call } from "./core";

/** 模型行（与后端 `LlmModelRow` 同构，snake_case；不含密钥本体）。 */
export interface LlmModelRow {
  id: string;
  /** 用户填写的显示名称，可空。 */
  display_name: string;
  /** 有效显示名：显示名 trim 后非空则用它，否则回退为模型名称。 */
  effective_name: string;
  base_url: string;
  model: string;
  api_key_configured: boolean;
}

/** 列出模型表。 */
export function llmListModels(): Promise<LlmModelRow[]> {
  return call<LlmModelRow[]>("llm_list_models");
}

/** 新增模型，返回新生成的 id。 */
export function llmAddModel(
  displayName: string,
  baseUrl: string,
  model: string,
): Promise<string> {
  return call<string>("llm_add_model", { displayName, baseUrl, model });
}

/** 按 id 更新模型。 */
export function llmUpdateModel(
  id: string,
  displayName: string,
  baseUrl: string,
  model: string,
): Promise<void> {
  return call<void>("llm_update_model", { id, displayName, baseUrl, model });
}

/** 按 id 删除模型（同时清除其密钥）。 */
export function llmRemoveModel(id: string): Promise<void> {
  return call<void>("llm_remove_model", { id });
}

/** 写入某条模型的 API Key（不回显）。 */
export function llmSetApiKey(id: string, apiKey: string): Promise<void> {
  return call<void>("llm_set_api_key", { id, apiKey });
}

/** 清除某条模型的 API Key。 */
export function llmClearApiKey(id: string): Promise<void> {
  return call<void>("llm_clear_api_key", { id });
}
