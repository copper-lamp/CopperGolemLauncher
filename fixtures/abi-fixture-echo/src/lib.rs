//! 测试夹具插件：真实编译为动态库，用于端到端验证 helper 的加载与调用链路。
//!
//! 它覆盖三类可观测行为，使集成测试能断言"链路真的通了"而不只是"没有报错"：
//! 1. `init` 收到的宿主配置会被原样回显（证明 config 通道可用）；
//! 2. 生命周期回调按顺序被调用（证明 helper 真的在驱动插件，而非空转）；
//! 3. 能力请求会拿到宿主状态码（当前应被显式拒绝，证明 fail closed 生效）。

use std::ffi::c_void;

use copper_module_abi::plugin_abi::{
    AbiBuffer, AbiBytes, HostApi, ABI_STATUS_ERROR, ABI_STATUS_OK,
};

/// 插件实例状态。
struct EchoState {
    /// 宿主在 `init` 时下发的配置原文，`invoke` 时回显。
    config: Vec<u8>,
    /// 宿主 API。指针由宿主持有，生命周期覆盖整个插件实例。
    host: *const HostApi,
    started: bool,
    stopped: bool,
    /// 最近一次能力请求得到的宿主状态码。
    last_capability_status: i32,
}

unsafe extern "C" fn init(
    host: *const HostApi,
    config: AbiBytes,
    state_out: *mut *mut c_void,
) -> i32 {
    if host.is_null() || state_out.is_null() {
        return ABI_STATUS_ERROR;
    }
    let host_ref = unsafe { &*host };
    if host_ref.validate().is_err() {
        return ABI_STATUS_ERROR;
    }

    let config = match unsafe { abi_bytes_to_vec(config) } {
        Some(config) => config,
        None => return ABI_STATUS_ERROR,
    };

    let state = Box::new(EchoState {
        config,
        host,
        started: false,
        stopped: false,
        last_capability_status: ABI_STATUS_OK,
    });
    unsafe { *state_out = Box::into_raw(state) as *mut c_void };
    ABI_STATUS_OK
}

unsafe extern "C" fn start(state: *mut c_void) -> i32 {
    match unsafe { (state as *mut EchoState).as_mut() } {
        Some(state) => {
            state.started = true;
            ABI_STATUS_OK
        }
        None => ABI_STATUS_ERROR,
    }
}

unsafe extern "C" fn stop(state: *mut c_void) -> i32 {
    match unsafe { (state as *mut EchoState).as_mut() } {
        Some(state) => {
            state.stopped = true;
            ABI_STATUS_OK
        }
        None => ABI_STATUS_ERROR,
    }
}

unsafe extern "C" fn destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state as *mut EchoState) });
    }
}

/// 支持的命令：
/// - `demo.echo`：回显 `args`，并附带插件收到的配置与启动状态；
/// - `demo.state`：返回插件侧生命周期与最近一次能力请求的状态码；
/// - `demo.probe_capability`：向宿主发起一次能力请求并返回其状态码。
unsafe extern "C" fn invoke(
    state: *mut c_void,
    operation: AbiBytes,
    input: AbiBytes,
    output: *mut AbiBuffer,
) -> i32 {
    let Some(state) = (unsafe { (state as *mut EchoState).as_mut() }) else {
        return ABI_STATUS_ERROR;
    };
    if output.is_null() {
        return ABI_STATUS_ERROR;
    }
    let output = unsafe { &mut *output };

    let Some(command_bytes) = (unsafe { abi_bytes_to_vec(operation) }) else {
        return ABI_STATUS_ERROR;
    };
    let Ok(command) = String::from_utf8(command_bytes) else {
        return ABI_STATUS_ERROR;
    };
    let Some(input) = (unsafe { abi_bytes_to_vec(input) }) else {
        return ABI_STATUS_ERROR;
    };
    let args: serde_json::Value = serde_json::from_slice(&input).unwrap_or(serde_json::Value::Null);

    let response = match command.as_str() {
        "demo.echo" => serde_json::json!({
            "echo": args,
            "config": String::from_utf8_lossy(&state.config),
            "started": state.started,
        }),
        "demo.state" => serde_json::json!({
            "started": state.started,
            "stopped": state.stopped,
            "last_capability_status": state.last_capability_status,
        }),
        "demo.probe_capability" => {
            let (status, payload) = unsafe { request_capability(state, "world.read") };
            state.last_capability_status = status;
            let payload: serde_json::Value =
                serde_json::from_slice(&payload).unwrap_or(serde_json::Value::Null);
            serde_json::json!({ "capability_status": status, "capability_payload": payload })
        }
        other => serde_json::json!({ "unsupported_command": other }),
    };

    let encoded = match serde_json::to_vec(&response) {
        Ok(encoded) => encoded,
        Err(_) => return ABI_STATUS_ERROR,
    };
    write_output(output, &encoded)
}

/// 通过宿主 API 发起一次能力请求，返回宿主状态码与宿主写回的结果原文。
///
/// 返回原文（而不是只返回状态码）才能让集成测试断言"结果真的经输出缓冲回到了
/// 插件"，而不只是"调用没有报错"。
unsafe fn request_capability(state: &mut EchoState, capability: &str) -> (i32, Vec<u8>) {
    let Some(host) = (unsafe { state.host.as_ref() }) else {
        return (ABI_STATUS_ERROR, Vec::new());
    };
    let Some(callback) = host.call_capability else {
        return (ABI_STATUS_ERROR, Vec::new());
    };

    let empty = AbiBytes {
        ptr: std::ptr::null(),
        len: 0,
    };
    let capability = AbiBytes {
        ptr: capability.as_ptr(),
        len: capability.len() as u64,
    };
    let mut buffer = [0_u8; 1024];
    let mut output = AbiBuffer {
        ptr: buffer.as_mut_ptr(),
        capacity: buffer.len() as u64,
        len: 0,
    };

    let status = unsafe { callback(host.user_data, capability, empty, &mut output) };
    let written = usize::try_from(output.len)
        .unwrap_or(0)
        .min(buffer.len());
    (status, buffer[..written].to_vec())
}

/// 把字节写入宿主提供的输出缓冲；容量不足时按协议上报需求长度。
fn write_output(output: &mut AbiBuffer, bytes: &[u8]) -> i32 {
    let required = bytes.len() as u64;
    if output.ptr.is_null() || output.capacity < required {
        output.len = required;
        return copper_module_abi::plugin_abi::ABI_STATUS_BUFFER_TOO_SMALL;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), output.ptr, bytes.len());
    }
    output.len = required;
    ABI_STATUS_OK
}

/// 读取宿主/自身提供的字节块。拷贝而非借用：跨 ABI 的裸指针无法携带生命周期，
/// 拷贝是唯一安全的选择。
unsafe fn abi_bytes_to_vec(bytes: AbiBytes) -> Option<Vec<u8>> {
    if bytes.len == 0 {
        return Some(Vec::new());
    }
    if bytes.ptr.is_null() {
        return None;
    }
    let len = usize::try_from(bytes.len).ok()?;
    Some(unsafe { std::slice::from_raw_parts(bytes.ptr, len) }.to_vec())
}

copper_module_abi::copper_module_plugin! {
    id = "copper-lamp.demo-tools",
    init = init,
    start = start,
    invoke = invoke,
    stop = stop,
    destroy = destroy,
}
