//! 插件侧导出宏：插件只需写业务回调，由宏生成入口符号与函数表回填。
//!
//! 插件作者不应手工拼 `PluginFunctionTable`：结构布局、版本号、模块 id 编码
//! 任何一处写错都会导致"能被扫描但不被加载"的隐蔽故障。宏把这些固定下来，
//! 只暴露五个回调。
//!
//! ```ignore
//! copper_module_abi::copper_module_plugin! {
//!     id = "author.module",
//!     init = my_init,
//!     start = my_start,
//!     invoke = my_invoke,
//!     stop = my_stop,
//!     destroy = my_destroy,
//! }
//! ```

/// 生成 `copper_module_plugin_entry` 入口。
///
/// 五个回调都必须是 `unsafe extern "C"` 函数，签名见
/// [`crate::plugin_abi::InitCallback`] 等类型别名。
#[macro_export]
macro_rules! copper_module_plugin {
    (
        id = $id:literal,
        init = $init:path,
        start = $start:path,
        invoke = $invoke:path,
        stop = $stop:path,
        destroy = $destroy:path $(,)?
    ) => {
        /// 插件入口。宿主与 helper 只通过这个符号发现插件。
        #[no_mangle]
        pub unsafe extern "C" fn copper_module_plugin_entry(
            table: *mut $crate::plugin_abi::PluginFunctionTable,
        ) -> i32 {
            if table.is_null() {
                return $crate::plugin_abi::ABI_STATUS_ERROR;
            }
            let table = unsafe { &mut *table };
            table.struct_size =
                ::std::mem::size_of::<$crate::plugin_abi::PluginFunctionTable>() as u32;
            table.abi_version = $crate::plugin_abi::ABI_VERSION;
            match $crate::plugin_abi::ModuleId::new($id) {
                Ok(module_id) => table.module_id = module_id,
                Err(_) => return $crate::plugin_abi::ABI_STATUS_ERROR,
            }
            table.init = ::core::option::Option::Some($init);
            table.start = ::core::option::Option::Some($start);
            table.invoke = ::core::option::Option::Some($invoke);
            table.stop = ::core::option::Option::Some($stop);
            table.destroy = ::core::option::Option::Some($destroy);
            $crate::plugin_abi::ABI_STATUS_OK
        }
    };
}
