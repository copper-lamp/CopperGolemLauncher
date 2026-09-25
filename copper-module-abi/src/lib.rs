//! 附加模块稳定契约层。
//!
//! 本 crate 是内核与附加模块之间**唯一的**共享边界，被三方复用：
//!
//! - 铜内核：作为宿主，按 [`ipc`] 协议驱动 helper 进程并校验插件产物；
//! - `copper-module-helper`：独立进程，按 [`plugin_abi`] 加载并调用插件；
//! - 附加模块模板：只依赖本 crate 即可实现对 ABI，无需链接内核（更不涉及 Tauri）。
//!
//! 拆成独立 crate 而不是留在内核里，是因为插件产物**不应该**为了拿到 ABI 类型
//! 而链接整个启动器：那会带来体积、启动开销和平台依赖污染。
//!
//! 三层版本边界彼此独立，禁止合并成一个版本号：
//! [`ipc::PROTOCOL_VERSION`]（内核 ↔ helper）、[`plugin_abi::ABI_VERSION`]
//! （helper ↔ 插件）、以及模块清单自身的 schema 版本。

pub mod helper_client;
pub mod helper_runtime;
pub mod ipc;
pub mod module_id;
pub mod plugin_abi;
pub mod plugin_export;
