//! 联动中介：事件总线（广播）+ 意图注册表（请求 / 响应）+ 模块契约与注册表 + 模块沙箱。
//!
//! 附加模块装载链路的文件分工：
//! - [`manifest`]：`module.json` 的内核侧契约与校验；
//! - [`module_entry`]：动态库入口符号契约与导出宏（ABI 触点，已被 [`helper_backend`] 取代）；
//! - [`loader`]：装载后端抽象 + 扫描 / 校验 / 注册编排；
//! - [`backend_router`]：按清单是否声明 `runtime` 把模块分流到两条装载后端；
//! - [`helper_backend`]：受监管子进程装载后端（当前实现）：IPC 会话 + [`modules::Module`] 代理；
//! - [`node_backend`]：受监管 Node 会话装载后端：派生 Agent 进程并驱动其生命周期；
//! - [`capability`]：插件能力请求的宿主派发（身份取自会话绑定，未知能力 fail closed）；
//! - [`module_storage`]：模块私有存储（命名空间由会话身份决定）；
//! - [`addon_events`]：事件总线的推送桥（有界队列 + 每模块推送线程，见其文件注释）；
//! - [`package`]：`.cglm` 包的安全解包与原子落位；
//! - [`install`]：安装链路（下载 → 校验 → 解包 → 落位）；
//! - [`frontend`]：前端产物自定义协议服务与入口列举。

pub mod addon_events;
pub mod backend_router;
pub mod capability;
pub mod dylib_backend;
pub mod events;
pub mod frontend;
pub mod helper_backend;
pub mod install;
pub mod intents;
pub mod loader;
pub mod manifest;
pub mod module_entry;
pub mod module_storage;
pub mod modules;
pub mod node_backend;
pub mod runtime;
pub mod package;
pub mod sandbox;

// 插件 ABI 与宿主 IPC 契约位于独立 crate `copper-module-abi`：内核、隔离的 helper
// 进程与附加模块模板共享同一份定义。此处 re-export 保持 `registry::ipc` /
// `registry::plugin_abi` 路径稳定。
pub use copper_module_abi::{ipc, plugin_abi};
