//! `copper-module-helper`：在独立进程中加载并驱动**单个**附加模块插件。
//!
//! 契约与职责：
//! - stdin 只接收宿主请求帧，stdout 只写出响应帧（长度前缀 JSON）。
//!   **stdout 不允许出现任何日志**，诊断信息一律走 stderr。
//! - 每个 helper 进程只服务一个模块，进程退出即回收该模块的全部资源。
//! - 宿主关闭管道视为正常结束；协议错误以非零退出码报告，便于 supervisor 区分
//!   "正常停机"与"协议故障"。
//!
//! 启动参数：`--module-id <id> --plugin <path>`

use std::io;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use copper_module_abi::helper_runtime::{
    validate_module_id, validate_plugin_path, HelperChannel, HelperRuntime,
};
use copper_module_abi::ipc::{IpcError, IpcMessage};

/// 参数或前置校验失败。
const EXIT_USAGE: u8 = 2;
/// 协议读写或帧解析失败。
const EXIT_PROTOCOL: u8 = 3;

fn main() -> ExitCode {
    let (module_id, plugin_path) = match parse_args() {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("[helper] {message}");
            eprintln!("[helper] usage: copper-module-helper --module-id <id> --plugin <path>");
            return ExitCode::from(EXIT_USAGE);
        }
    };

    if let Err(failure) = validate_module_id(&module_id) {
        eprintln!("[helper] {}", failure.message);
        return ExitCode::from(EXIT_USAGE);
    }
    if let Err(failure) = validate_plugin_path(&plugin_path) {
        eprintln!("[helper] {}", failure.message);
        return ExitCode::from(EXIT_USAGE);
    }

    let channel = Arc::new(Mutex::new(HelperChannel::stdio()));
    let mut runtime = HelperRuntime::with_channel(module_id, plugin_path, Arc::clone(&channel));

    // 单调循环：每轮读一条宿主消息、处理、回一条响应。
    //
    // 关键约束：**处理请求期间不持通道锁**。插件在 `invoke` 内发起能力请求时，
    // 它的回调要借用同一个通道；若此处持锁，双方会死锁。
    loop {
        let message = {
            let mut guard = lock_channel(&channel);
            match guard.recv() {
                Ok(message) => message,
                // 宿主关闭管道：正常停机，不视为错误。
                Err(IpcError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    return ExitCode::SUCCESS;
                }
                Err(error) => {
                    eprintln!("[helper] failed to read a host frame: {error}");
                    return ExitCode::from(EXIT_PROTOCOL);
                }
            }
        };

        let request = match message {
            IpcMessage::Request(request) => request,
            IpcMessage::Response(response) => {
                eprintln!(
                    "[helper] host sent an unexpected response for `{}`",
                    response.request_id
                );
                return ExitCode::from(EXIT_PROTOCOL);
            }
        };

        let response = runtime.handle(request);
        if let Err(error) = lock_channel(&channel).send(&IpcMessage::Response(response)) {
            eprintln!("[helper] failed to write a response frame: {error}");
            return ExitCode::from(EXIT_PROTOCOL);
        }
    }
}

fn lock_channel(channel: &Arc<Mutex<HelperChannel>>) -> std::sync::MutexGuard<'_, HelperChannel> {
    channel.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn parse_args() -> Result<(String, std::path::PathBuf), String> {
    let mut module_id: Option<String> = None;
    let mut plugin: Option<String> = None;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--module-id" => module_id = args.next(),
            "--plugin" => plugin = args.next(),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }

    let module_id = module_id.ok_or_else(|| "--module-id is required".to_owned())?;
    let plugin = plugin.ok_or_else(|| "--plugin is required".to_owned())?;
    Ok((module_id, std::path::PathBuf::from(plugin)))
}
