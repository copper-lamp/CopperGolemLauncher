//! 附加模块动态库的入口契约（导出符号 + 构造宏）。
//!
//! 附加模块当前以**同进程动态库**装载（见 [`crate::registry::dylib_backend`]）。
//! 本文件定义内核与模块之间的唯一 ABI 触点：模块动态库必须导出一个名为
//! [`ENTRY_SYMBOL`] 的 C 符号，返回指向 `Arc<dyn Module>` 的裸指针。
//!
//! # ABI 约束（诚实记录，属已知风险）
//!
//! Rust 没有稳定 ABI，本契约成立的前提是**模块动态库与内核使用同一份
//! `copper-core` crate、同一编译器、同一优化/panic 配置构建**。这不是「任意
//! 第三方二进制即插即用」的稳定 ABI——那是受管控子进程 + IPC 方案的职责。
//! 因此当前阶段只应装载官方模块；跨版本 / 跨工具链的二进制会被拒绝或崩溃。
//!
//! 另需注意内核 `release` profile 为 `panic = "abort"`：模块动态库若未按同样
//! 配置构建，panic 展开方式不一致会导致未定义行为。这条同样要求「同配置构建」。
//!
//! 未来切换到受管控子进程时，本文件被 IPC 传输层取代，
//! [`crate::registry::loader::ModuleLoadBackend`] 之上的调用方无需改动。

use crate::registry::modules::Module;

/// 模块动态库必须导出的唯一符号名（`\0` 结尾，供 `libloading` 查找）。
pub const ENTRY_SYMBOL: &[u8] = b"copper_module_entry\0";

/// 入口函数签名：返回 `Arc<dyn Module>` 的裸指针（由 `Arc::into_raw` 产生）。
///
/// 返回**肥指针**（trait 对象）而非裸 `*mut ()`：内核侧
/// [`crate::registry::dylib_backend`] 直接 `Arc::from_raw` 取回所有权，
/// 引用计数在两侧对称，避免「模块侧泄漏、内核侧又克隆一份」。
#[allow(improper_ctypes_definitions)]
pub type ModuleEntryFn = extern "C" fn() -> *const dyn Module;

/// 为模块类型生成标准入口符号。
///
/// 用法（模块 crate 的 `lib.rs`）：
///
/// ```ignore
/// copper_core_lib::copper_module_entry!(copper_module_demo::DemoModule);
/// ```
///
/// 要求模块类型实现 [`Default`]；内核在 `Arc::new(Default::default())` 后接管所有权。
#[macro_export]
macro_rules! copper_module_entry {
    ($ty:path) => {
        /// 模块入口：内核经 `libloading` 查找并调用。
        ///
        /// 符号名固定为 `copper_module_entry`，是内核与模块之间唯一的 ABI 触点。
        #[no_mangle]
        #[allow(improper_ctypes_definitions)]
        pub extern "C" fn copper_module_entry() -> *const dyn $crate::registry::modules::Module {
            ::std::sync::Arc::into_raw(
                ::std::sync::Arc::new(<$ty as ::std::default::Default>::default())
                    as ::std::sync::Arc<dyn $crate::registry::modules::Module>,
            )
        }
    };
}
