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
use copper_module_abi::ipc::{IpcError, IpcMessage, IpcRequest};

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

    pump(&channel, &mut runtime)
}

/// 主循环：读一帧 → 处理 → 需要时回帧（单向通知不回）→ 排空被延迟的宿主通知。
///
/// 顺序是刻意的：延迟队列里的条目是**上面这次请求的处理过程**产生的——插件在
/// `invoke` 内发起能力请求时，宿主推来的事件会被能力回调暂存。因此必须先处理完
/// 当前请求再排空，否则会与正在进行的插件调用重入。
fn pump(channel: &Arc<Mutex<HelperChannel>>, runtime: &mut HelperRuntime) -> ExitCode {
    loop {
        let request = match read_host_request(channel) {
            Ok(request) => request,
            Err(code) => return code,
        };

        if let Err(code) = deliver(channel, runtime, request) {
            return code;
        }
        for deferred in runtime.drain_deferred() {
            if let Err(code) = deliver(channel, runtime, deferred) {
                return code;
            }
        }
    }
}

/// 读一帧宿主请求。宿主关闭管道视为正常停机（helper 没有别的事可做）。
fn read_host_request(channel: &Arc<Mutex<HelperChannel>>) -> Result<IpcRequest, ExitCode> {
    let message = {
        let mut guard = lock_channel(channel);
        match guard.recv() {
            Ok(message) => message,
            Err(IpcError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(ExitCode::SUCCESS);
            }
            Err(error) => {
                eprintln!("[helper] failed to read a host frame: {error}");
                return Err(ExitCode::from(EXIT_PROTOCOL));
            }
        }
    };

    match message {
        IpcMessage::Request(request) => Ok(request),
        IpcMessage::Response(response) => {
            eprintln!(
                "[helper] host sent an unexpected response for `{}`",
                response.request_id
            );
            Err(ExitCode::from(EXIT_PROTOCOL))
        }
    }
}

/// 处理一帧宿主请求：请求 / 响应方法回帧，单向通知回 `None`（不回帧）。
fn deliver(
    channel: &Arc<Mutex<HelperChannel>>,
    runtime: &mut HelperRuntime,
    request: IpcRequest,
) -> Result<(), ExitCode> {
    let Some(response) = runtime.handle_host_frame(request) else {
        // 单向通知：宿主没有人在等响应帧。多写一帧会被宿主当成某个 invoke 的响应，
        // 把一次成功的调用判成 request id 不匹配失败。
        return Ok(());
    };
    if let Err(error) = lock_channel(channel).send(&IpcMessage::Response(response)) {
        eprintln!("[helper] failed to write a response frame: {error}");
        return Err(ExitCode::from(EXIT_PROTOCOL));
    }
    Ok(())
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
