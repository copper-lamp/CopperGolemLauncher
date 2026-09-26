// LLM 配置 API：状态、保存非敏感配置、写入 / 清除密钥。
//
// 内核极简——只存 base URL / 模型名 / API Key 三项，不预设 provider 或 base URL。
// 密钥走系统密钥环且绝不回显：`llm_status` 只回「是否已配置」。

import { call } from "./core";

/** LLM 配置状态（与后端 `LlmStatus` 同构，snake_case；不含密钥本体）。 */
export interface LlmStatus {
  base_url: string;
  model: string;
  api_key_configured: boolean;
}

/** 读取 LLM 配置状态。 */
export function llmStatus(): Promise<LlmStatus> {
  return call<LlmStatus>("llm_status");
}

/** 保存非敏感配置（base URL / 模型名）。 */
export function llmSaveConfig(baseUrl: string, model: string): Promise<void> {
  return call<void>("llm_save_config", { baseUrl, model });
}

/** 写入 API Key（不回显）。 */
export function llmSetApiKey(apiKey: string): Promise<void> {
  return call<void>("llm_set_api_key", { apiKey });
}

/** 清除 API Key。 */
export function llmClearApiKey(): Promise<void> {
  return call<void>("llm_clear_api_key");
}
