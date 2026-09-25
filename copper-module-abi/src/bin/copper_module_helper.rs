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

use std::io::{self, BufReader, BufWriter, Write};
use std::process::ExitCode;

use copper_module_abi::helper_runtime::{validate_module_id, validate_plugin_path, HelperRuntime};
use copper_module_abi::ipc::{read_frame, write_frame, IpcError, IpcRequest};

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

    let mut runtime = HelperRuntime::new(module_id, plugin_path);
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());

    loop {
        let payload = match read_frame(&mut reader) {
            Ok(payload) => payload,
            // 宿主关闭管道：正常停机，不视为错误。
            Err(IpcError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("[helper] failed to read a host frame: {error}");
                return ExitCode::from(EXIT_PROTOCOL);
            }
        };

        let request: IpcRequest = match serde_json::from_slice(&payload) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("[helper] host sent a malformed request frame: {error}");
                return ExitCode::from(EXIT_PROTOCOL);
            }
        };

        let response = runtime.handle(request);
        let encoded = match serde_json::to_vec(&response) {
            Ok(encoded) => encoded,
            Err(error) => {
                eprintln!("[helper] failed to encode a response: {error}");
                return ExitCode::from(EXIT_PROTOCOL);
            }
        };

        if let Err(error) = write_frame(&mut writer, &encoded) {
            eprintln!("[helper] failed to write a response frame: {error}");
            return ExitCode::from(EXIT_PROTOCOL);
        }
        // 必须逐帧刷新：宿主在等这一帧，缓冲会让双方互相等待。
        if let Err(error) = writer.flush() {
            eprintln!("[helper] failed to flush a response frame: {error}");
            return ExitCode::from(EXIT_PROTOCOL);
        }
    }
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
