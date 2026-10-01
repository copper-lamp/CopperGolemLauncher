//! Android 日志后端：把内核日志与 panic 送回 `logcat`。
//!
//! # 为什么必须单独做这件事
//!
//! 内核日志原本只有两层出口：stdout 与 `<data>/logs/kernel.log`。两者在 Android 上都
//! 不可靠：
//!
//! - **stdout 不进 logcat**：真机实测（SHARK PAR-A0 / Android 12）里，`RustStdoutStderr`
//!   标签下只看到 WebView 自己的 stderr，内核的 `println!` 一行都没有；
//! - **文件日志在启动早期可能不落盘**：挂载日志文件发生在 `Paths::prepare()` 之后，
//!   而真机上出现过「进程起来了、`copper.db` 被写过、但 `kernel.log` 停在旧时间」的
//!   现场——需要日志的那一段恰好没日志。
//!
//! 结果是最难定位的一类故障：`setup()` 没跑完（`app.manage(kernel)` 未执行，所有命令
//! 以 `state not managed` 失败），但**没有任何一行输出能说明断在哪**。
//!
//! 因此这里补第三条出口：经 JNI 调 `android.util.Log.println`，Android 上永远可达。
//!
//! # 不变量
//!
//! - **桌面零影响**：整个模块仅在 `target_os = "android"` 下编译；
//! - **绝不让日志成为启动失败的原因**：取不到 `JavaVM`、attach 失败、JNI 调用异常
//!   一律静默降级（真机没日志很糟，但比开不了机好）；
//! - **不引入 `ndk-context`**：它在未初始化时直接 panic，而本项目的依赖链里没有
//!   任何一方调用 `initialize_android_context`（见 `Cargo.toml` 的 Android 段落）。
//!   这里改用 JNI 规范里的 `JNI_GetCreatedJavaVMs` 自取句柄，不依赖外部初始化。

use std::sync::OnceLock;

use jni::objects::JValue;
use jni::sys::jint;
use jni::{JNIEnv, JavaVM};

/// logcat 单条消息的上限。超长文本（例如带 backtrace 的 panic）会被截断，
/// 避免单条日志撑爆 logcat 缓冲区把前因后果一起挤掉。
const MAX_MESSAGE_CHARS: usize = 3500;

/// JNI 规范函数：取进程中已创建的 JavaVM。
///
/// 签名来自 `jni.h`：`jint JNI_GetCreatedJavaVMs(JavaVM **vmBuf, jsize bufLen, jsize *nVMs)`。
/// 符号由 Android 运行时/`libnativehelper` 提供，`jni` crate 没有导出它，故此处自行声明。
unsafe extern "C" {
    fn JNI_GetCreatedJavaVMs(vm_buf: *mut *mut jni::sys::JavaVM, buf_len: i32, n_vms: *mut i32) -> i32;
}

/// 缓存的 JavaVM。取一次即可，JVM 生命周期覆盖整个进程。
static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

/// 获取（并缓存）JavaVM；失败返回 `None`。
fn java_vm() -> Option<&'static JavaVM> {
    if let Some(vm) = JAVA_VM.get() {
        return Some(vm);
    }
    let mut raw: *mut jni::sys::JavaVM = std::ptr::null_mut();
    let mut count: jint = 0;
    // SAFETY: 参数指向本函数栈上的有效变量；失败时不会写入 raw。
    let rc = unsafe { JNI_GetCreatedJavaVMs(&mut raw, 1, &mut count) };
    if rc != 0 || count < 1 || raw.is_null() {
        return None;
    }
    // SAFETY: raw 来自 JNI 运行时，指向进程级 JavaVM，且非空（上面已判空）。
    let vm = unsafe { JavaVM::from_raw(raw) }.ok()?;
    let _ = JAVA_VM.set(vm);
    JAVA_VM.get()
}

/// 把一行文本写入 logcat。
///
/// 取不到 JavaVM 或 attach 失败时**静默返回**：日志不可用只能降级，不能反过来
/// 让调用方（含 `log::Log::log` 与 panic 钩子）跟着失败。
pub fn write(level: log::Level, target: &str, message: &str) {
    let Some(vm) = java_vm() else { return };
    // `attach_current_thread` 在本线程未附着时附着，Drop 时自动 detach；
    // 内核有多个后台线程（下载、事件推送）会走这里，故用作用域附着而非永久附着。
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let priority = match level {
        log::Level::Error => 6, // android.util.Log.ERROR
        log::Level::Warn => 5,  // WARN
        log::Level::Info => 4,  // INFO
        log::Level::Debug => 3, // DEBUG
        log::Level::Trace => 2, // VERBOSE
    };
    let _ = println_line(&mut env, priority, target, message);
}

/// 单次 `Log.println(priority, tag, msg)` 调用。
fn println_line(env: &mut JNIEnv, priority: jint, tag: &str, message: &str) -> jni::errors::Result<()> {
    let tag = truncate(tag, 64);
    let message = truncate(message, MAX_MESSAGE_CHARS);
    let class = env.find_class("android/util/Log")?;
    let tag = env.new_string(tag)?;
    let message = env.new_string(message)?;
    env.call_static_method(
        class,
        "println",
        "(ILjava/lang/String;Ljava/lang/String;)I",
        &[
            JValue::Int(priority),
            JValue::Object(&tag),
            JValue::Object(&message),
        ],
    )?;
    Ok(())
}

/// 按字符边界截断，避免把多字节字符切坏（中文日志很常见）。
fn truncate(text: &str, max_chars: usize) -> &str {
    if text.chars().count() <= max_chars {
        return text;
    }
    match text.char_indices().nth(max_chars) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

/// 尝试写一条自检日志，用于确认后端是否真的接上了 logcat。
///
/// 独立成一个函数：启动早期调用一次，出问题时能立刻分辨「后端没接上」与
/// 「后端接上了但后续没有日志」——这两种情况的排查方向完全不同。
pub fn probe() {
    write(
        log::Level::Info,
        "CopperKernel",
        "Android 日志后端已接入（经 android.util.Log.println）",
    );
}
