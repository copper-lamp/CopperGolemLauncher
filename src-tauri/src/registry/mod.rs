//! 联动中介：事件总线（广播）+ 意图注册表（请求 / 响应）+ 模块契约与注册表 + 模块沙箱。
//!
//! 附加模块装载链路的文件分工：
//! - [`manifest`]：`module.json` 的内核侧契约与校验；
//! - [`module_entry`]：动态库入口符号契约与导出宏（ABI 触点）；
//! - [`loader`]：装载后端抽象 + 扫描 / 校验 / 注册编排；
//! - [`dylib_backend`]：同进程动态库装载后端（当前实现）；
//! - [`package`]：`.cglm` 包的安全解包与原子落位；
//! - [`install`]：安装链路（下载 → 校验 → 解包 → 落位）；
//! - [`frontend`]：前端产物自定义协议服务与入口列举。

pub mod dylib_backend;
pub mod events;
pub mod frontend;
pub mod install;
pub mod intents;
pub mod ipc;
pub mod loader;
pub mod manifest;
pub mod module_entry;
pub mod modules;
pub mod package;
pub mod sandbox;
