// 模块 API：模块清单、启用 / 禁用。

import { call } from "./core";

export type ModuleState = "stopped" | "running" | "failed";

/** 模块信息（供设置页"模块"Tab 展示）。 */
export interface ModuleInfo {
  id: string;
  enabled: boolean;
  is_builtin: boolean;
  state: ModuleState;
  error: string | null;
}

/** 模块信息列表。 */
export function modulesList(): Promise<ModuleInfo[]> {
  return call<ModuleInfo[]>("modules_list");
}

/** 切换模块启用状态（下次启动生效）。 */
export function modulesSetEnabled(id: string, enabled: boolean): Promise<void> {
  return call<void>("modules_set_enabled", { id, enabled });
}
