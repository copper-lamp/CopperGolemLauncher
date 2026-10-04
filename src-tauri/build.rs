// SPDX-License-Identifier: GPL-3.0-only
//
//! 构建脚本：Tauri 代码生成 + 应用清单。
//!
//! ## 平台判定必须用「目标」而不是 `cfg(windows)`
//!
//! 这个文件是**构建脚本**，永远由**宿主**编译器编译。因此在 Windows 上做
//! `cargo build --target aarch64-linux-android` 时，`cfg(windows)` 仍然为真——
//! 用它会走进「编译并链接 Windows 资源」的分支，产出的 COFF `.lib` 被塞进安卓链接
//! 命令，lld 直接报 `unknown file type`，表现为 `could not compile copper-core`。
//!
//! 所以下面一律用 `cfg!(target_os = "windows")` 与 cargo 提供的
//! `CARGO_CFG_TARGET_OS`（它们描述的是**目标**平台）来判断。

/// 本次构建的目标平台是否为 Windows。
///
/// 构建脚本里 `cfg!(target_os = ...)` 对**宿主**求值（cargo 用宿主的 rustc 编译构建
/// 脚本，且不会为目标平台设置 `--cfg`），所以唯一可靠的事实源是 cargo 注入的
/// `CARGO_CFG_TARGET_OS`。
fn target_is_windows() -> bool {
    std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
}

/// Common Controls v6 应用清单内容。
///
/// 依赖链里的 `muda` / `tao` 引用了 `comctl32!TaskDialogIndirect`，该导出只存在于
/// Common Controls v6。没有声明 v6 依赖的 PE 会让系统把 `comctl32.dll` 解析到 v5，
/// 加载器随即以 `0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND` 终止进程。表现是
/// `cargo check --workspace` 完全正常，`cargo test --workspace` 却一条测试都跑不起来
/// （测试二进制根本没机会执行）。
///
/// 内容与 `tauri-build` 自带的清单逐字一致。
///
/// 无条件编译：本文件的 `cfg` 描述的都是宿主，而这份清单只在**目标**是 Windows 时才
/// 使用（判断见 `target_is_windows`）。宿主非 Windows 时它是死代码。
#[allow(dead_code)]
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
///
/// 无条件编译，由 `main` 用 `target_is_windows()` 决定是否调用（原因见文件头：
/// 这里的 `cfg` 只描述宿主，而「要不要嵌 Windows 资源」取决于目标）。
#[allow(dead_code)]
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

/// 定位 hook DLL（`copper-core-hook` 的 cdylib 产物）并把绝对路径交给编译期。
///
/// 内核启动游戏前要把这个 DLL 写进实例目录，所以字节必须**编进二进制**：
/// 运行时去 target 目录找产物，在发布安装包里不存在。
///
/// DLL 缺失即构建失败：没有 DLL = 没有隔离、没有加载器注入，功能等于
/// 悄悄关掉。那种「构建过了但用户发现资源装了不起」的失败比构建失败难查得多。
///
/// 目标平台非 Windows 时不产出路径（隔离与原生预加载是 Windows GDK 专属）。
fn export_hook_dll_path() {
    if !target_is_windows() {
        return;
    }
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("cargo 必须提供 CARGO_MANIFEST_DIR"),
    );
    // 允许用环境变量覆盖，便于 IDE / CI 指向自定义产物目录。
    let dir = match std::env::var("COPPER_HOOK_DLL_DIR") {
        Ok(custom) => std::path::PathBuf::from(custom),
        Err(_) => {
            let target_triple = std::env::var("TARGET").unwrap_or_else(|_| {
                // 交叉编译时 TARGET 一定存在；这里只是给出可读的失败信息。
                "unknown-target".to_string()
            });
            // 用本次构建的 profile，而不是写死 release：`cargo check` / `cargo test`
            // 走 debug，产物也在 debug 目录。写死 release 会让最常用的开发命令
            // 直接失败。
            let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
            manifest_dir
                .join("..")
                .join("native")
                .join("hook")
                .join("target")
                .join(target_triple)
                .join(profile)
        }
    };
    let dll = dir.join("copper_core_hook.dll");
    if !dll.is_file() {
        panic!(
            "找不到 hook DLL：{}\n\
             该 DLL 由 `native/hook` 产出，内核在启动游戏前需要把它写进实例目录。\n\
             请先构建：cargo build -p copper-core-hook --release",
            dll.display()
        );
    }
    println!("cargo:rustc-env=COPPER_HOOK_DLL={}", dll.display());
    // 变更 DLL 产物本身也要触发重新编译（include_bytes! 的依赖追踪对绝对路径有效，
    // 这里显式声明一次以防目录调整后失效）。
    println!("cargo:rerun-if-changed={}", dll.display());
}

fn main() {
    // 只在目标平台是 Windows 时嵌清单：构建脚本跑在宿主上，用 `cfg(windows)` 会在
    // 「Windows 宿主 + 安卓目标」时错误地嵌进一份 COFF 资源库（见文件头说明）。
    if target_is_windows() {
        embed_app_manifest();
        export_hook_dll_path();
    }
    // `new_without_app_manifest()` 对所有平台都给：它关掉的是 `tauri-build` 自己那份
    // `RT_MANIFEST`。本 crate 的 Windows 资源由上面的 `embed_app_manifest()` 独占提供，
    // 所以这条不会造成重复；而在移动端它同时避免 tauri-build 往链接参数里塞资源。
    use tauri_build::{Attributes, WindowsAttributes};
    let attributes =
        Attributes::new().windows_attributes(WindowsAttributes::new_without_app_manifest());
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build 失败: {error:#}");
    }
}