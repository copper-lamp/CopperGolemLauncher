// 模块 API：模块清单、启用 / 禁用。
//
// 内核命令见 `src-tauri/src/commands/modules.rs`。
// `modules_list` 返回的 `ModuleInfo` 正处于扩展期（cgl-libs.md 差距项 G2 / G6）：
// 除既有字段外，内核侧将补充 `version` / `source` / `display_name` / `dir`。
// 为容忍内核尚未合并的中间状态，新增字段一律声明为可选，前端不得假设其存在。

import { call } from "./core";

export type ModuleState = "stopped" | "running" | "failed";

/** 模块来源（G2：`is_builtin` 恒为 `true` 的替代标记）。 */
export type ModuleSource = "builtin" | "installed" | string;

/** 模块信息（供设置页"模块"Tab 展示）。 */
export interface ModuleInfo {
  /** 模块 id，与 cgl-libs 条目 `id` 逐字相同，用于关联远端元数据。 */
  id: string;
  enabled: boolean;
  /** 是否为内核内置模块；旧内核恒为 `true`。 */
  is_builtin: boolean;
  state: ModuleState;
  error: string | null;
  /** 是否已被沙箱停权（既有字段）。 */
  suspended?: boolean;
  /** 本地已装版本（semver）；内置模块由内核提供，未合并时为 `undefined`。 */
  version?: string;
  /** 来源标记；未合并时前端按 `is_builtin` 推断。 */
  source?: ModuleSource;
  /** 内核侧展示名；优先于远端条目的 `display_name`。 */
  display_name?: string;
  /** 模块安装目录绝对路径。 */
  dir?: string;
}

/** 模块信息列表。 */
export function modulesList(): Promise<ModuleInfo[]> {
  return call<ModuleInfo[]>("modules_list");
}

/** 切换模块启用状态（下次启动生效）。 */
export function modulesSetEnabled(id: string, enabled: boolean): Promise<void> {
  return call<void>("modules_set_enabled", { id, enabled });
}

/** 模块是否为内置（容忍内核侧 `source` 尚未合并的情况）。 */
export function isBuiltinModule(module: ModuleInfo): boolean {
  if (module.source !== undefined) return module.source === "builtin";
  return module.is_builtin;
}
