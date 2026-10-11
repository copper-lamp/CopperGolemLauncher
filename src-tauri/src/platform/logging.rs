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
//! 因此这里补第三条出口：直接调 `liblog` 的 `__android_log_write` 写 logcat。
//!
//! # 为什么不用 JNI（真机踩过的坑，别再回去）
//!
//! 这条出口最初是「经 JNI 调 `android.util.Log.println`」，并且为了自取 `JavaVM`
//! 直接引用了 JNI 规范里的 `JNI_GetCreatedJavaVMs`。真机上该写法**必然启动崩溃**：
//!
//! ```text
//! UnsatisfiedLinkError: dlopen failed: cannot locate symbol
//! "JNI_GetCreatedJavaVMs" referenced by libcopper_core_lib.so
//!   at com.copperlamp.coppergolem.Rust.<clinit>(Rust.kt:18)
//!   at com.copperlamp.coppergolem.WryActivity.onCreate(WryActivity.kt:26)
//! ```
//!
//! 该符号只由系统私有库 `libnativehelper.so` 提供，普通 app 的 linker namespace 里
//! 不可见，于是 `System.loadLibrary` 直接失败、Activity 跟着崩——**日志后端反过来成了
//! 启动失败的原因**，正是本模块最不能容忍的事。更麻烦的是 `cargo check` / `cargo build`
//! 全都发现不了：前者不链接，后者在 Android 上允许 .so 带未定义符号，只有真机 `dlopen`
//! 才炸。要拿 `JavaVM`，本项目的既定姿势是 [`crate::platform::keystore`] 里的
//! `JNI_OnLoad`（JVM 加载 .so 时回调），不要自己去找运行时符号。
//!
//! `liblog.so` 一定可达：它是 app 可见的公开 NDK 库，且已在 `libcopper_core_lib.so`
//! 的 `DT_NEEDED` 里。`__android_log_write` 同样落到 logcat、优先级语义一致，还省掉了
//! attach/detach 线程与 Java 层调用。
//!
//! # 不变量
//!
//! - **桌面零影响**：整个模块仅在 `target_os = "android"` 下编译；
//! - **绝不让日志成为启动失败的原因**：只使用 `liblog` 的公开入口，不引入任何可能
//!   解析失败的符号；写入本身无失败路径，静默降级即可（真机没日志很糟，但比开不了机好）。

use std::ffi::CString;
use std::os::raw::{c_char, c_int};

/// logcat 单条消息的上限。超长文本（例如带 backtrace 的 panic）会被截断，
/// 避免单条日志撑爆 logcat 缓冲区把前因后果一起挤掉。
const MAX_MESSAGE_CHARS: usize = 3500;

/// `android/log.h` 的优先级常量（与 `android.util.Log` 一致）。
const ANDROID_LOG_VERBOSE: c_int = 2;
const ANDROID_LOG_DEBUG: c_int = 3;
const ANDROID_LOG_INFO: c_int = 4;
const ANDROID_LOG_WARN: c_int = 5;
const ANDROID_LOG_ERROR: c_int = 6;

unsafe extern "C" {
    /// 签名来自 `android/log.h`：
    /// `int __android_log_write(int prio, const char* tag, const char* text)`。
    /// 符号由 `liblog.so` 提供（已在 `DT_NEEDED` 中），app 一定解析得到。
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// 把一行文本写入 logcat。
pub fn write(level: log::Level, target: &str, message: &str) {
    let priority = match level {
        log::Level::Error => ANDROID_LOG_ERROR,
        log::Level::Warn => ANDROID_LOG_WARN,
        log::Level::Info => ANDROID_LOG_INFO,
        log::Level::Debug => ANDROID_LOG_DEBUG,
        log::Level::Trace => ANDROID_LOG_VERBOSE,
    };
    // logcat 的 tag 以短名为宜，这里统一截到 64 字符。
    let tag = cstring(truncate(target, 64));
    let text = cstring(truncate(message, MAX_MESSAGE_CHARS));
    // SAFETY: 两个 CString 都活在本函数栈上、生命周期覆盖此次调用，liblog 只读取它们；
    // 返回值（写入字节数或负错误码）对调用方没有可操作含义，忽略。
    unsafe { __android_log_write(priority, tag.as_ptr(), text.as_ptr()) };
}

/// 转成 C 字符串。
///
/// 日志文本里出现内嵌 NUL 只可能来自二进制内容；这种情况替换成可见转义，
/// 而不是把整条日志丢掉。
fn cstring(text: &str) -> CString {
    CString::new(text.replace('\0', "\\0")).unwrap_or_default()
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
        "Android 日志后端已接入（经 __android_log_write）",
    );
}
