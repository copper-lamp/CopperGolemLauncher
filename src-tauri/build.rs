// SPDX-License-Identifier: GPL-3.0-only
//
//! 构建脚本：Tauri 代码生成 + 应用清单。

/// Common Controls v6 应用清单内容。
///
/// 依赖链里的 `muda` / `tao` 引用了 `comctl32!TaskDialogIndirect`，该导出只存在于
/// Common Controls v6。没有声明 v6 依赖的 PE 会让系统把 `comctl32.dll` 解析到 v5，
/// 加载器随即以 `0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND` 终止进程。表现是
/// `cargo check --workspace` 完全正常，`cargo test --workspace` 却一条测试都跑不起来
/// （测试二进制根本没机会执行）。
///
/// 内容与 `tauri-build` 自带的清单逐字一致。
#[cfg(windows)]
const APP_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

/// 编译应用清单资源，并把它链接到**本 crate 的所有目标**。
///
/// 为什么要接管清单的所有权：`tauri-build` 走 `WindowsResource::compile()`，而该
/// 函数只在「本 crate 带 bin 目标」时发 `cargo:rustc-link-arg-bins`——只覆盖可执行
/// 体。测试二进制 `deps/copper_core_lib-*.exe` 因此完全没有资源段，加载期即失败。
///
/// 为什么不用 `cargo:rustc-link-arg-tests`（语义上最贴切）：Cargo 1.98 直接以
/// `invalid instruction` 拒绝该指令（已用最小 crate 复现），`cargo:` 与 `cargo::`
/// 两种写法都一样，构建会硬失败。
///
/// 为什么让 `tauri-build` 不再自带清单：`rustc-link-arg` 覆盖全部目标，如果
/// `tauri-build` 再嵌一份自己的 `RT_MANIFEST`，同一个可执行体里就会出现两个同名资源，
/// `cvtres` 直接报 `CVT1100: 资源重复`。因此这里用
/// `WindowsAttributes::new_without_app_manifest()` 关掉它自带的那份，由本脚本提供
/// 唯一一份——所有目标（可执行体、cdylib、测试）共享同一个资源对象，既不重复也不遗漏。
///
/// 注意：`embed-resource` 自己的 `compile()` 只有在**没有** bin 目标时才退回全目标
/// 链接，本 crate 有 `src/main.rs`，所以那条路走不通。
#[cfg(windows)]
fn embed_app_manifest() {
    let out_dir =
        std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo 必须提供 OUT_DIR"));
    let manifest_file = out_dir.join("copper-core-app-manifest.xml");
    let resource_file = out_dir.join("copper-core-app-manifest.rc");
    if let Err(error) = std::fs::write(&manifest_file, APP_MANIFEST) {
        panic!("写入应用清单失败: {error}");
    }
    // 资源 ID 1 / 类型 24（RT_MANIFEST）。
    let content = format!(
        "1 24 \"{}\"\n",
        manifest_file.display().to_string().replace('\\', "\\\\")
    );
    if let Err(error) = std::fs::write(&resource_file, content) {
        panic!("写入资源脚本失败: {error}");
    }

    // 自己驱动资源编译，拿到确定的 .lib 路径后再自己发链接参数。
    //
    // 刻意用 `compile_for` 而不是 `compile`：后者会自己再发一条
    // `cargo:rustc-link-arg-bins`，与下面这条全局指令叠加后，同一个可执行体里会
    // 出现两份同名 `RT_MANIFEST`，`cvtres` 直接报 `CVT1100: 资源重复`。
    let object = out_dir.join("copper-core-app-manifest.lib");
    let rc = embed_resource::compile_for(&resource_file, std::iter::empty::<&str>(), embed_resource::NONE);
    if let Err(error) = rc.manifest_required() {
        // 失败必须显式爆出来：悄悄跳过只会退化成加载期 0xc0000139，
        // 而那个现象正是本函数要消除的、最难定位的失败模式。
        panic!("编译应用清单资源失败（需要 Windows SDK 的 rc.exe）: {error:?}");
    }
    // 全局指令：覆盖可执行体、cdylib、示例、测试与基准，主程序那份已由
    // `WindowsAttributes::new_without_app_manifest()` 让出，因此不会重复。
    println!("cargo:rustc-link-arg={}", object.display());
}

#[cfg(not(windows))]
fn embed_app_manifest() {}

fn main() {
    embed_app_manifest();
    #[cfg(windows)]
    {
        use tauri_build::{Attributes, WindowsAttributes};
        let attributes =
            Attributes::new().windows_attributes(WindowsAttributes::new_without_app_manifest());
        if let Err(error) = tauri_build::try_build(attributes) {
            panic!("tauri-build 失败: {error:#}");
        }
    }
    #[cfg(not(windows))]
    tauri_build::build()
}