use std::ffi::c_void;
use std::fmt;

use crate::module_id::is_valid_module_id;

pub const ABI_VERSION: u32 = 1;
pub const MODULE_ID_CAPACITY: usize = 128;
pub const MAX_ABI_BUFFER_BYTES: usize = 8 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbiBytes {
    pub ptr: *const u8,
    pub len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbiBuffer {
    pub ptr: *mut u8,
    pub capacity: u64,
    pub len: u64,
}

impl AbiBytes {
    pub fn validate_input(&self, max_bytes: usize) -> Result<usize, AbiValidationError> {
        validate_abi_range(self.ptr.is_null(), self.len, max_bytes)
    }
}

impl AbiBuffer {
    pub fn validate_output(&self, max_bytes: usize) -> Result<usize, AbiValidationError> {
        validate_abi_range(self.ptr.is_null(), self.capacity, max_bytes)
    }
}

fn validate_abi_range(
    is_null: bool,
    length: u64,
    max_bytes: usize,
) -> Result<usize, AbiValidationError> {
    let length = usize::try_from(length).map_err(|_| AbiValidationError::BufferTooLarge {
        size: u64::MAX,
        max: max_bytes,
    })?;
    if length > max_bytes {
        return Err(AbiValidationError::BufferTooLarge {
            size: length as u64,
            max: max_bytes,
        });
    }
    if is_null && length != 0 {
        return Err(AbiValidationError::NullPointerWithNonzeroLength);
    }
    Ok(length)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleId {
    pub bytes: [u8; MODULE_ID_CAPACITY],
    pub len: u32,
}

impl ModuleId {
    pub fn new(value: &str) -> Result<Self, AbiValidationError> {
        if !is_valid_module_id(value) {
            return Err(AbiValidationError::InvalidModuleId);
        }
        let bytes = value.as_bytes();
        if bytes.len() > MODULE_ID_CAPACITY {
            return Err(AbiValidationError::InvalidModuleId);
        }
        let mut encoded = [0; MODULE_ID_CAPACITY];
        encoded[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            bytes: encoded,
            len: bytes.len() as u32,
        })
    }

    pub fn as_str(&self) -> Result<&str, AbiValidationError> {
        if self.len > MODULE_ID_CAPACITY as u32 {
            return Err(AbiValidationError::InvalidModuleId);
        }
        let len = self.len as usize;
        let value = std::str::from_utf8(&self.bytes[..len])
            .map_err(|_| AbiValidationError::InvalidModuleId)?;
        if !is_valid_module_id(value) {
            return Err(AbiValidationError::InvalidModuleId);
        }
        Ok(value)
    }
}

pub type InitCallback =
    unsafe extern "C" fn(*const HostApi, AbiBytes, *mut *mut c_void) -> i32;
pub type StartCallback = unsafe extern "C" fn(*mut c_void) -> i32;
pub type InvokeCallback =
    unsafe extern "C" fn(*mut c_void, AbiBytes, AbiBytes, *mut AbiBuffer) -> i32;
pub type StopCallback = unsafe extern "C" fn(*mut c_void) -> i32;
pub type DestroyCallback = unsafe extern "C" fn(*mut c_void);
pub type CallCapabilityCallback =
    unsafe extern "C" fn(*mut c_void, AbiBytes, AbiBytes, *mut AbiBuffer) -> i32;
pub type PluginEntry = unsafe extern "C" fn(*mut PluginFunctionTable) -> i32;

pub const PLUGIN_ENTRY_SYMBOL: &[u8] = b"copper_module_plugin_entry\0";
pub const ABI_STATUS_OK: i32 = 0;
pub const ABI_STATUS_ERROR: i32 = 1;
pub const ABI_STATUS_BUFFER_TOO_SMALL: i32 = 2;
/// 宿主明确拒绝或尚未实现该能力。插件不得据此猜测成功。
pub const ABI_STATUS_NOT_SUPPORTED: i32 = 3;

#[repr(C)]
pub struct HostApi {
    pub struct_size: u32,
    pub abi_version: u32,
    pub user_data: *mut c_void,
    pub call_capability: Option<CallCapabilityCallback>,
}

impl HostApi {
    pub fn validate(&self) -> Result<(), AbiValidationError> {
        if self.struct_size as usize != std::mem::size_of::<Self>() {
            return Err(AbiValidationError::InvalidStructSize(self.struct_size));
        }
        if self.abi_version != ABI_VERSION {
            return Err(AbiValidationError::UnsupportedVersion(self.abi_version));
        }
        if self.call_capability.is_none() {
            return Err(AbiValidationError::MissingCallback("host.call_capability"));
        }
        if self.user_data.is_null() {
            return Err(AbiValidationError::MissingHostContext);
        }
        Ok(())
    }
}

#[repr(C)]
pub struct PluginFunctionTable {
    pub struct_size: u32,
    pub abi_version: u32,
    pub module_id: ModuleId,
    pub init: Option<InitCallback>,
    pub start: Option<StartCallback>,
    pub invoke: Option<InvokeCallback>,
    pub stop: Option<StopCallback>,
    pub destroy: Option<DestroyCallback>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbiValidationError {
    UnsupportedVersion(u32),
    MissingCallback(&'static str),
    InvalidStructSize(u32),
    InvalidModuleId,
    MissingHostContext,
    NullPointerWithNonzeroLength,
    BufferTooLarge { size: u64, max: usize },
    ModuleIdMismatch { expected: String, actual: String },
}

impl fmt::Display for AbiValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => write!(f, "unsupported plugin ABI version {version}"),
            Self::MissingCallback(callback) => write!(f, "missing required callback {callback}"),
            Self::InvalidStructSize(size) => write!(f, "invalid plugin function table size {size}"),
            Self::InvalidModuleId => f.write_str("invalid plugin module id"),
            Self::MissingHostContext => f.write_str("missing host-bound plugin context"),
            Self::NullPointerWithNonzeroLength => {
                f.write_str("null ABI pointer with non-zero length")
            }
            Self::BufferTooLarge { size, max } => {
                write!(f, "ABI buffer size {size} exceeds maximum {max}")
            }
            Self::ModuleIdMismatch { expected, actual } => {
                write!(f, "plugin module id mismatch: expected {expected}, got {actual}")
            }
        }
    }
}

impl std::error::Error for AbiValidationError {}

impl PluginFunctionTable {
    pub fn validate(&self, expected_module_id: &str) -> Result<(), AbiValidationError> {
        ModuleId::new(expected_module_id)?;
        if self.struct_size as usize != std::mem::size_of::<Self>() {
            return Err(AbiValidationError::InvalidStructSize(self.struct_size));
        }
        if self.abi_version != ABI_VERSION {
            return Err(AbiValidationError::UnsupportedVersion(self.abi_version));
        }
        let actual = self.module_id.as_str()?.to_owned();
        if actual != expected_module_id {
            return Err(AbiValidationError::ModuleIdMismatch {
                expected: expected_module_id.to_owned(),
                actual,
            });
        }
        if self.init.is_none() {
            return Err(AbiValidationError::MissingCallback("init"));
        }
        if self.start.is_none() {
            return Err(AbiValidationError::MissingCallback("start"));
        }
        if self.invoke.is_none() {
            return Err(AbiValidationError::MissingCallback("invoke"));
        }
        if self.stop.is_none() {
            return Err(AbiValidationError::MissingCallback("stop"));
        }
        if self.destroy.is_none() {
            return Err(AbiValidationError::MissingCallback("destroy"));
        }
        Ok(())
    }
}

/// 调用插件回调时的错误。
#[derive(Debug)]
pub enum AbiCallError {
    /// 函数表或宿主 API 未通过 ABI 校验。
    Validation(AbiValidationError),
    /// 插件入口返回失败状态。
    EntryFailed(i32),
    /// 插件初始化返回失败状态。
    InitFailed(i32),
    /// 插件启动返回失败状态。
    StartFailed(i32),
    /// 插件停止返回失败状态。
    StopFailed(i32),
    /// 插件回调返回未定义状态码。
    CallbackFailed(i32),
    /// 插件违反 ABI 契约（成功状态下输出长度越界等）。
    ContractViolation(&'static str),
    /// 插件反复索取缓冲却始终不足，宿主放弃继续扩容。
    OutputRetriesExhausted { attempts: usize },
}

impl fmt::Display for AbiCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => write!(f, "plugin ABI validation failed: {error}"),
            Self::EntryFailed(status) => write!(f, "plugin entry returned status {status}"),
            Self::InitFailed(status) => write!(f, "plugin init returned status {status}"),
            Self::StartFailed(status) => write!(f, "plugin start returned status {status}"),
            Self::StopFailed(status) => write!(f, "plugin stop returned status {status}"),
            Self::CallbackFailed(status) => write!(f, "plugin callback returned status {status}"),
            Self::ContractViolation(detail) => write!(f, "plugin contract violation: {detail}"),
            Self::OutputRetriesExhausted { attempts } => {
                write!(f, "plugin output buffer still too small after {attempts} attempts")
            }
        }
    }
}

impl std::error::Error for AbiCallError {}

impl From<AbiValidationError> for AbiCallError {
    fn from(error: AbiValidationError) -> Self {
        Self::Validation(error)
    }
}

/// 首次交给插件的输出缓冲容量；不足时按插件上报的需求扩容。
const INITIAL_OUTPUT_CAPACITY: usize = 512;
/// 输出缓冲扩容重试上限，防止插件诱导无限扩容。
const MAX_OUTPUT_ATTEMPTS: usize = 8;

/// 宿主侧持有的插件实例：函数表 + 插件状态 + 宿主 API 与宿主上下文。
///
/// # 所有权与生命周期
///
/// - `host` 与 `context` 由本结构拥有，二者地址稳定，且 **保证在 `destroy`
///   被调用之前仍然有效**：插件在 init 时取得 `&HostApi` 并可能长期保存该指针，
///   能力回调也依赖 `user_data` 指向的宿主上下文。
/// - `state` 由插件在 init 中产出，只能经 `destroy` 归还。本结构在 drop 时若
///   尚未归还会自动补一次 destroy，避免插件资源泄漏。
/// - 模块身份由宿主通过 `expected_module_id` 与 `context` 绑定，插件无法声明或
///   改写自身身份。
pub struct PluginInstance<C> {
    table: PluginFunctionTable,
    state: *mut c_void,
    host: Box<HostApi>,
    context: Box<C>,
}

impl<C> PluginInstance<C> {
    /// 加载并初始化一个插件实例：调用入口取函数表 → 校验 → 用宿主上下文初始化。
    ///
    /// # Safety
    ///
    /// `entry` 必须来自本进程已按同一 ABI 版本编译并校验过的插件产物。ABI 只能
    /// 约束结构与协议，无法阻止恶意插件让 helper 崩溃，因此插件进程隔离仍是必要
    /// 的兜底。
    pub unsafe fn instantiate(
        entry: PluginEntry,
        expected_module_id: &str,
        config: &[u8],
        context: C,
        call_capability: CallCapabilityCallback,
    ) -> Result<Self, AbiCallError> {
        if config.len() > MAX_ABI_BUFFER_BYTES {
            return Err(AbiValidationError::BufferTooLarge {
                size: config.len() as u64,
                max: MAX_ABI_BUFFER_BYTES,
            }
            .into());
        }
        // 显式给出全 None 的初值，插件未填写的字段会被校验拒绝，而不是被当作已就绪。
        let mut table = PluginFunctionTable {
            struct_size: 0,
            abi_version: 0,
            module_id: ModuleId {
                bytes: [0; MODULE_ID_CAPACITY],
                len: 0,
            },
            init: None,
            start: None,
            invoke: None,
            stop: None,
            destroy: None,
        };
        let status = unsafe { entry(&mut table) };
        if status != ABI_STATUS_OK {
            return Err(AbiCallError::EntryFailed(status));
        }
        table.validate(expected_module_id)?;

        let mut context = Box::new(context);
        let host = Box::new(HostApi {
            struct_size: std::mem::size_of::<HostApi>() as u32,
            abi_version: ABI_VERSION,
            user_data: context.as_mut() as *mut C as *mut c_void,
            call_capability: Some(call_capability),
        });
        host.validate()?;

        let mut state: *mut c_void = std::ptr::null_mut();
        let init = table
            .init
            .ok_or(AbiValidationError::MissingCallback("init"))?;
        let config = AbiBytes {
            ptr: config.as_ptr(),
            len: config.len() as u64,
        };
        let status = unsafe { init(host.as_ref(), config, &mut state) };
        if status != ABI_STATUS_OK {
            return Err(AbiCallError::InitFailed(status));
        }
        if state.is_null() {
            return Err(AbiCallError::ContractViolation(
                "init reported success but returned a null instance state",
            ));
        }

        Ok(Self {
            table,
            state,
            host,
            context,
        })
    }

    /// 宿主上下文（会话绑定的模块身份与能力派发所需状态）。
    pub fn context(&self) -> &C {
        &self.context
    }

    /// 交给插件的宿主 API。`user_data` 始终指向本实例的宿主上下文。
    pub fn host(&self) -> &HostApi {
        &self.host
    }

    /// 调用插件 `start`。
    pub fn start(&mut self) -> Result<(), AbiCallError> {
        let callback = self
            .table
            .start
            .ok_or(AbiValidationError::MissingCallback("start"))?;
        let status = unsafe { callback(self.state) };
        if status == ABI_STATUS_OK {
            Ok(())
        } else {
            Err(AbiCallError::StartFailed(status))
        }
    }

    /// 调用插件 `invoke`，按插件上报的需求扩容输出缓冲并重试。
    pub fn invoke(&mut self, operation: &str, input: &[u8]) -> Result<Vec<u8>, AbiCallError> {
        if operation.len() > MAX_ABI_BUFFER_BYTES || input.len() > MAX_ABI_BUFFER_BYTES {
            return Err(AbiValidationError::BufferTooLarge {
                size: operation.len().max(input.len()) as u64,
                max: MAX_ABI_BUFFER_BYTES,
            }
            .into());
        }
        let invoke = self.invoke_callback()?;
        self.call_with_growing_buffer(
            invoke,
            AbiBytes {
                ptr: operation.as_ptr(),
                len: operation.len() as u64,
            },
            AbiBytes {
                ptr: input.as_ptr(),
                len: input.len() as u64,
            },
        )
    }

    fn invoke_callback(&self) -> Result<InvokeCallback, AbiCallError> {
        self.table
            .invoke
            .ok_or_else(|| AbiValidationError::MissingCallback("invoke").into())
    }

    fn call_with_growing_buffer(
        &mut self,
        invoke: InvokeCallback,
        operation: AbiBytes,
        input: AbiBytes,
    ) -> Result<Vec<u8>, AbiCallError> {
        let mut capacity = INITIAL_OUTPUT_CAPACITY;
        for _ in 0..MAX_OUTPUT_ATTEMPTS {
            let mut buffer = vec![0_u8; capacity];
            let mut output = AbiBuffer {
                ptr: buffer.as_mut_ptr(),
                capacity: buffer.len() as u64,
                len: 0,
            };
            let status = unsafe { invoke(self.state, operation, input, &mut output) };
            match status {
                ABI_STATUS_OK => {
                    if output.len > output.capacity {
                        return Err(AbiCallError::ContractViolation(
                            "callback reported success with an output length above the buffer capacity",
                        ));
                    }
                    let len = usize::try_from(output.len).map_err(|_| {
                        AbiCallError::ContractViolation(
                            "callback reported an unrepresentable length",
                        )
                    })?;
                    buffer.truncate(len);
                    return Ok(buffer);
                }
                ABI_STATUS_BUFFER_TOO_SMALL => {
                    let requested = usize::try_from(output.len).map_err(|_| {
                        AbiValidationError::BufferTooLarge {
                            size: u64::MAX,
                            max: MAX_ABI_BUFFER_BYTES,
                        }
                    })?;
                    if requested > MAX_ABI_BUFFER_BYTES {
                        return Err(AbiValidationError::BufferTooLarge {
                            size: output.len,
                            max: MAX_ABI_BUFFER_BYTES,
                        }
                        .into());
                    }
                    if requested <= capacity {
                        return Err(AbiCallError::ContractViolation(
                            "callback requested a larger buffer without reporting a larger length",
                        ));
                    }
                    capacity = requested;
                }
                other => return Err(AbiCallError::CallbackFailed(other)),
            }
        }
        Err(AbiCallError::OutputRetriesExhausted {
            attempts: MAX_OUTPUT_ATTEMPTS,
        })
    }

    /// 调用插件 `stop`。
    pub fn stop(&mut self) -> Result<(), AbiCallError> {
        let callback = self
            .table
            .stop
            .ok_or(AbiValidationError::MissingCallback("stop"))?;
        let status = unsafe { callback(self.state) };
        if status == ABI_STATUS_OK {
            Ok(())
        } else {
            Err(AbiCallError::StopFailed(status))
        }
    }

    /// 归还插件实例状态。可重复调用；第二次起为空操作。
    pub fn destroy(&mut self) {
        if self.state.is_null() {
            return;
        }
        if let Some(callback) = self.table.destroy {
            unsafe { callback(self.state) };
        }
        self.state = std::ptr::null_mut();
    }
}

impl<C> Drop for PluginInstance<C> {
    fn drop(&mut self) {
        // Drop body 先于字段释放执行，因此此时 host/context 仍然有效。
        self.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn init(
        host: *const HostApi,
        _config: AbiBytes,
        state_out: *mut *mut c_void,
    ) -> i32 {
        if host.is_null() || state_out.is_null() {
            return ABI_STATUS_ERROR;
        }
        let host = unsafe { &*host };
        if host.validate().is_err() {
            return ABI_STATUS_ERROR;
        }
        unsafe {
            *state_out = host as *const HostApi as *mut c_void;
        }
        ABI_STATUS_OK
    }

    unsafe extern "C" fn start(_state: *mut c_void) -> i32 {
        0
    }

    unsafe extern "C" fn invoke(
        _state: *mut c_void,
        _operation: AbiBytes,
        _input: AbiBytes,
        output: *mut AbiBuffer,
    ) -> i32 {
        if output.is_null() {
            return ABI_STATUS_ERROR;
        }
        let output = unsafe { &mut *output };
        if output.capacity < 2 || output.ptr.is_null() {
            output.len = 2;
            return ABI_STATUS_BUFFER_TOO_SMALL;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(b"ok".as_ptr(), output.ptr, 2);
        }
        output.len = 2;
        ABI_STATUS_OK
    }

    unsafe extern "C" fn verify_host_context(
        host_context: *mut c_void,
        _capability: AbiBytes,
        _input: AbiBytes,
        _output: *mut AbiBuffer,
    ) -> i32 {
        if host_context.is_null() || unsafe { *(host_context as *const u8) } != 23 {
            return ABI_STATUS_ERROR;
        }
        ABI_STATUS_OK
    }

    unsafe extern "C" fn call_capability(
        _state: *mut c_void,
        _capability: AbiBytes,
        _input: AbiBytes,
        output: *mut AbiBuffer,
    ) -> i32 {
        if output.is_null() {
            return ABI_STATUS_ERROR;
        }
        let output = unsafe { &mut *output };
        if output.capacity < 2 || output.ptr.is_null() {
            output.len = 2;
            return ABI_STATUS_BUFFER_TOO_SMALL;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(b"ok".as_ptr(), output.ptr, 2);
        }
        output.len = 2;
        ABI_STATUS_OK
    }

    unsafe extern "C" fn stop(_state: *mut c_void) -> i32 {
        0
    }

    unsafe extern "C" fn destroy(_state: *mut c_void) {}

    fn table(module_id: &str) -> PluginFunctionTable {
        PluginFunctionTable {
            struct_size: std::mem::size_of::<PluginFunctionTable>() as u32,
            abi_version: ABI_VERSION,
            module_id: ModuleId::new(module_id).unwrap(),
            init: Some(init),
            start: Some(start),
            invoke: Some(invoke),
            stop: Some(stop),
            destroy: Some(destroy),
        }
    }

    fn host_api() -> HostApi {
        HostApi {
            struct_size: std::mem::size_of::<HostApi>() as u32,
            abi_version: ABI_VERSION,
            user_data: std::ptr::NonNull::<u8>::dangling().as_ptr() as *mut c_void,
            call_capability: Some(call_capability),
        }
    }

    #[test]
    fn invoke_callback_can_return_output_length_to_the_host() {
        let mut output_bytes = [0; 4];
        let mut output = AbiBuffer {
            ptr: output_bytes.as_mut_ptr(),
            capacity: output_bytes.len() as u64,
            len: 0,
        };
        let invoke = table("copper-lamp.demo-tools").invoke.unwrap();
        let empty = AbiBytes {
            ptr: std::ptr::null(),
            len: 0,
        };

        let status = unsafe { invoke(std::ptr::null_mut(), empty, empty, &mut output) };

        assert_eq!(status, ABI_STATUS_OK);
        assert_eq!(output.len, 2);
        let output_len = usize::try_from(output.len).unwrap();
        assert_eq!(&output_bytes[..output_len], b"ok");
    }

    #[test]
    fn invoke_callback_reports_required_output_capacity() {
        let mut output = AbiBuffer {
            ptr: std::ptr::null_mut(),
            capacity: 0,
            len: 0,
        };
        let invoke = table("copper-lamp.demo-tools").invoke.unwrap();
        let empty = AbiBytes {
            ptr: std::ptr::null(),
            len: 0,
        };

        let status = unsafe { invoke(std::ptr::null_mut(), empty, empty, &mut output) };

        assert_eq!(status, ABI_STATUS_BUFFER_TOO_SMALL);
        assert_eq!(output.len, 2);
    }

    #[test]
    fn plugin_init_receives_host_api_and_preserves_host_bound_context() {
        let mut host = host_api();
        let mut host_context = 23_u8;
        host.user_data = &mut host_context as *mut _ as *mut c_void;
        host.call_capability = Some(verify_host_context);
        let mut state = std::ptr::null_mut();
        let plugin = table("copper-lamp.demo-tools");
        let empty = AbiBytes {
            ptr: std::ptr::null(),
            len: 0,
        };

        let status = unsafe { plugin.init.unwrap()(&host, empty, &mut state) };

        assert_eq!(status, ABI_STATUS_OK);
        assert_eq!(state, &host as *const HostApi as *mut c_void);
        let retained_host = unsafe { &*(state as *const HostApi) };
        let empty = AbiBytes {
            ptr: std::ptr::null(),
            len: 0,
        };
        assert_eq!(
            unsafe {
                retained_host.call_capability.unwrap()(
                    retained_host.user_data,
                    empty,
                    empty,
                    std::ptr::null_mut(),
                )
            },
            ABI_STATUS_OK
        );
    }

    #[test]
    fn host_api_rejects_missing_host_bound_context() {
        let mut host = host_api();
        host.user_data = std::ptr::null_mut();

        assert!(matches!(
            host.validate(),
            Err(AbiValidationError::MissingHostContext)
        ));
    }

    #[test]
    fn host_api_rejects_an_incompatible_struct_size() {
        let mut host = host_api();
        host.struct_size -= 1;

        assert!(matches!(
            host.validate(),
            Err(AbiValidationError::InvalidStructSize(_))
        ));
    }

    #[test]
    fn host_api_exposes_capability_rpc() {
        let host = host_api();
        assert!(host.call_capability.is_some());
    }

    #[test]
    fn capability_callback_receives_host_bound_context_not_plugin_identity() {
        let mut host_context = 23_u8;
        let mut host = host_api();
        host.user_data = &mut host_context as *mut _ as *mut c_void;
        host.call_capability = Some(verify_host_context);
        let empty = AbiBytes {
            ptr: std::ptr::null(),
            len: 0,
        };

        let status = unsafe {
            host.call_capability.unwrap()(
                host.user_data,
                empty,
                empty,
                std::ptr::null_mut(),
            )
        };

        assert_eq!(status, ABI_STATUS_OK);
    }

    #[test]
    fn capability_callback_returns_host_response_without_grant_bypass() {
        let mut response_bytes = [0; 4];
        let mut response = AbiBuffer {
            ptr: response_bytes.as_mut_ptr(),
            capacity: response_bytes.len() as u64,
            len: 0,
        };
        let callback = host_api().call_capability.unwrap();
        let capability = AbiBytes {
            ptr: b"world.read".as_ptr(),
            len: b"world.read".len() as u64,
        };
        let input = AbiBytes {
            ptr: b"{}".as_ptr(),
            len: 2,
        };

        let status = unsafe { callback(std::ptr::null_mut(), capability, input, &mut response) };

        assert_eq!(status, ABI_STATUS_OK);
        let response_len = usize::try_from(response.len).unwrap();
        assert_eq!(&response_bytes[..response_len], b"ok");
    }

    #[test]
    fn rejects_null_and_oversized_abi_buffers() {
        let invoke = table("copper-lamp.demo-tools").invoke.unwrap();
        let empty = AbiBytes {
            ptr: std::ptr::null(),
            len: 0,
        };
        let mut null_output = AbiBuffer {
            ptr: std::ptr::null_mut(),
            capacity: 2,
            len: 0,
        };
        assert_eq!(
            unsafe { invoke(std::ptr::null_mut(), empty, empty, &mut null_output) },
            ABI_STATUS_BUFFER_TOO_SMALL
        );

        let mut bytes = [0; 2];
        let mut output = AbiBuffer {
            ptr: bytes.as_mut_ptr(),
            capacity: bytes.len() as u64,
            len: u64::MAX,
        };
        assert_eq!(
            unsafe { invoke(std::ptr::null_mut(), empty, empty, &mut output) },
            ABI_STATUS_OK
        );
        assert_eq!(output.len, 2);
    }

    #[test]
    fn accepts_supported_abi_version_and_complete_callbacks() {
        assert!(table("copper-lamp.demo-tools")
            .validate("copper-lamp.demo-tools")
            .is_ok());
    }

    #[test]
    fn rejects_unsupported_abi_version() {
        let mut plugin = table("copper-lamp.demo-tools");
        plugin.abi_version = ABI_VERSION + 1;
        assert!(matches!(
            plugin.validate("copper-lamp.demo-tools"),
            Err(AbiValidationError::UnsupportedVersion(_))
        ));
    }

    #[test]
    fn rejects_missing_required_callback() {
        let mut plugin = table("copper-lamp.demo-tools");
        plugin.invoke = None;
        assert!(matches!(
            plugin.validate("copper-lamp.demo-tools"),
            Err(AbiValidationError::MissingCallback("invoke"))
        ));
    }

    #[test]
    fn rejects_invalid_module_id() {
        assert!(ModuleId::new("../escape").is_err());
        assert!(ModuleId::new("single").is_err());
        assert!(ModuleId::new(&"a".repeat(MODULE_ID_CAPACITY + 1)).is_err());
    }

    #[test]
    fn plugin_function_table_rejects_incompatible_struct_size() {
        let mut plugin = table("copper-lamp.demo-tools");
        plugin.struct_size -= 1;

        assert!(matches!(
            plugin.validate("copper-lamp.demo-tools"),
            Err(AbiValidationError::InvalidStructSize(_))
        ));

        let mut plugin = table("copper-lamp.demo-tools");
        plugin.struct_size += 1;

        assert!(matches!(
            plugin.validate("copper-lamp.demo-tools"),
            Err(AbiValidationError::InvalidStructSize(_))
        ));
    }

    #[test]
    fn abi_bytes_reject_nonzero_length_with_null_pointer() {
        let bytes = AbiBytes {
            ptr: std::ptr::null(),
            len: 1,
        };

        assert!(matches!(
            bytes.validate_input(MAX_ABI_BUFFER_BYTES),
            Err(AbiValidationError::NullPointerWithNonzeroLength)
        ));
    }

    #[test]
    fn abi_bytes_reject_payloads_above_the_limit() {
        let bytes = AbiBytes {
            ptr: std::ptr::NonNull::<u8>::dangling().as_ptr(),
            len: (MAX_ABI_BUFFER_BYTES + 1) as u64,
        };

        assert!(matches!(
            bytes.validate_input(MAX_ABI_BUFFER_BYTES),
            Err(AbiValidationError::BufferTooLarge { .. })
        ));
    }

    #[test]
    fn abi_buffer_rejects_nonzero_capacity_with_null_pointer() {
        let buffer = AbiBuffer {
            ptr: std::ptr::null_mut(),
            capacity: 4,
            len: 0,
        };

        assert!(matches!(
            buffer.validate_output(MAX_ABI_BUFFER_BYTES),
            Err(AbiValidationError::NullPointerWithNonzeroLength)
        ));
    }

    #[test]
    fn abi_buffer_lengths_use_fixed_width_fields() {
        let buffer = AbiBuffer {
            ptr: std::ptr::null_mut(),
            capacity: 0_u64,
            len: 0_u64,
        };

        assert_eq!(std::mem::size_of_val(&buffer.capacity), 8);
        assert_eq!(std::mem::size_of_val(&buffer.len), 8);
        assert_eq!(std::mem::size_of::<AbiBytes>(), std::mem::size_of::<*const u8>() + 8);
    }

    #[test]
    fn rejects_invalid_expected_module_id() {
        assert!(matches!(
            table("copper-lamp.demo-tools").validate("../escape"),
            Err(AbiValidationError::InvalidModuleId)
        ));
    }

    #[test]
    fn rejects_module_id_mismatch() {
        assert!(matches!(
            table("copper-lamp.demo-tools").validate("copper-lamp.other"),
            Err(AbiValidationError::ModuleIdMismatch { .. })
        ));
    }

    use std::cell::Cell;
    use std::rc::Rc;

    const FIXTURE_ID: &str = "copper-lamp.demo-tools";
    const FIXTURE_PAYLOAD: [u8; 1024] = [b'a'; 1024];

    /// fixture 插件的调用日志。宿主上下文即 `Rc<FixtureLog>`，
    /// 测试侧保留一份克隆即可观测插件行为。
    #[derive(Default)]
    struct FixtureLog {
        invokes: Cell<usize>,
        starts: Cell<usize>,
        stops: Cell<usize>,
        destroys: Cell<usize>,
        config_len: Cell<usize>,
    }

    /// fixture 插件实例状态：只保存宿主上下文指针，用于回写日志。
    struct FixtureState {
        host_ctx: *mut c_void,
    }

    fn fixture_log(ctx: *mut c_void) -> Rc<FixtureLog> {
        let log = unsafe { &*(ctx as *const Rc<FixtureLog>) };
        Rc::clone(log)
    }

    unsafe extern "C" fn fixture_init(
        host: *const HostApi,
        config: AbiBytes,
        state_out: *mut *mut c_void,
    ) -> i32 {
        if host.is_null() || state_out.is_null() {
            return ABI_STATUS_ERROR;
        }
        let host = unsafe { &*host };
        if host.validate().is_err() {
            return ABI_STATUS_ERROR;
        }
        let log = fixture_log(host.user_data);
        log.config_len.set(config.len as usize);
        let state = Box::new(FixtureState {
            host_ctx: host.user_data,
        });
        unsafe { *state_out = Box::into_raw(state) as *mut c_void };
        ABI_STATUS_OK
    }

    unsafe extern "C" fn fixture_start(state: *mut c_void) -> i32 {
        if state.is_null() {
            return ABI_STATUS_ERROR;
        }
        let state = unsafe { &*(state as *const FixtureState) };
        let log = fixture_log(state.host_ctx);
        log.starts.set(log.starts.get() + 1);
        ABI_STATUS_OK
    }

    unsafe extern "C" fn fixture_stop(state: *mut c_void) -> i32 {
        if state.is_null() {
            return ABI_STATUS_ERROR;
        }
        let state = unsafe { &*(state as *const FixtureState) };
        let log = fixture_log(state.host_ctx);
        log.stops.set(log.stops.get() + 1);
        ABI_STATUS_OK
    }

    unsafe extern "C" fn fixture_destroy(state: *mut c_void) {
        if state.is_null() {
            return;
        }
        let state = unsafe { Box::from_raw(state as *mut FixtureState) };
        let log = fixture_log(state.host_ctx);
        log.destroys.set(log.destroys.get() + 1);
    }

    /// 输出固定 1024 字节：大于宿主的初始缓冲容量，用于验证扩容重试。
    unsafe extern "C" fn fixture_invoke(
        state: *mut c_void,
        _operation: AbiBytes,
        _input: AbiBytes,
        output: *mut AbiBuffer,
    ) -> i32 {
        if state.is_null() || output.is_null() {
            return ABI_STATUS_ERROR;
        }
        let state = unsafe { &*(state as *const FixtureState) };
        let log = fixture_log(state.host_ctx);
        log.invokes.set(log.invokes.get() + 1);

        let output = unsafe { &mut *output };
        let required = FIXTURE_PAYLOAD.len() as u64;
        if output.ptr.is_null() || output.capacity < required {
            output.len = required;
            return ABI_STATUS_BUFFER_TOO_SMALL;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                FIXTURE_PAYLOAD.as_ptr(),
                output.ptr,
                FIXTURE_PAYLOAD.len(),
            );
        }
        output.len = required;
        ABI_STATUS_OK
    }

    /// 永远索取超过 ABI 上限的缓冲，宿主必须拒绝而不是无限扩容。
    unsafe extern "C" fn greedy_invoke(
        _state: *mut c_void,
        _operation: AbiBytes,
        _input: AbiBytes,
        output: *mut AbiBuffer,
    ) -> i32 {
        if output.is_null() {
            return ABI_STATUS_ERROR;
        }
        let output = unsafe { &mut *output };
        output.len = (MAX_ABI_BUFFER_BYTES + 1) as u64;
        ABI_STATUS_BUFFER_TOO_SMALL
    }

    /// 报告成功但把长度写成超过容量，宿主必须按契约违约拒绝，不能读取越界内容。
    unsafe extern "C" fn overreporting_invoke(
        _state: *mut c_void,
        _operation: AbiBytes,
        _input: AbiBytes,
        output: *mut AbiBuffer,
    ) -> i32 {
        if output.is_null() {
            return ABI_STATUS_ERROR;
        }
        let output = unsafe { &mut *output };
        output.len = output.capacity + 1;
        ABI_STATUS_OK
    }

    unsafe fn fill_fixture_table(table: *mut PluginFunctionTable, invoke: InvokeCallback) -> i32 {
        if table.is_null() {
            return ABI_STATUS_ERROR;
        }
        let out = unsafe { &mut *table };
        out.struct_size = std::mem::size_of::<PluginFunctionTable>() as u32;
        out.abi_version = ABI_VERSION;
        out.module_id = ModuleId::new(FIXTURE_ID).unwrap();
        out.init = Some(fixture_init);
        out.start = Some(fixture_start);
        out.invoke = Some(invoke);
        out.stop = Some(fixture_stop);
        out.destroy = Some(fixture_destroy);
        ABI_STATUS_OK
    }

    unsafe extern "C" fn fixture_entry(table: *mut PluginFunctionTable) -> i32 {
        unsafe { fill_fixture_table(table, fixture_invoke) }
    }

    unsafe extern "C" fn greedy_entry(table: *mut PluginFunctionTable) -> i32 {
        unsafe { fill_fixture_table(table, greedy_invoke) }
    }

    unsafe extern "C" fn overreporting_entry(table: *mut PluginFunctionTable) -> i32 {
        unsafe { fill_fixture_table(table, overreporting_invoke) }
    }

    unsafe extern "C" fn failing_entry(_table: *mut PluginFunctionTable) -> i32 {
        ABI_STATUS_ERROR
    }

    fn instance(
        entry: PluginEntry,
        log: &Rc<FixtureLog>,
    ) -> Result<PluginInstance<Rc<FixtureLog>>, AbiCallError> {
        unsafe {
            PluginInstance::instantiate(entry, FIXTURE_ID, &[], Rc::clone(log), call_capability)
        }
    }

    #[test]
    fn init_receives_host_supplied_config() {
        let log = Rc::new(FixtureLog::default());
        let config = br#"{"setting":1}"#;

        let _plugin = unsafe {
            PluginInstance::instantiate(
                fixture_entry,
                FIXTURE_ID,
                config,
                Rc::clone(&log),
                call_capability,
            )
        }
        .unwrap();

        assert_eq!(log.config_len.get(), config.len());
    }

    #[test]
    fn abi_status_codes_are_distinct() {
        let mut codes = vec![
            ABI_STATUS_OK,
            ABI_STATUS_ERROR,
            ABI_STATUS_BUFFER_TOO_SMALL,
            ABI_STATUS_NOT_SUPPORTED,
        ];
        codes.sort_unstable();
        codes.dedup();

        assert_eq!(codes.len(), 4);
    }

    #[test]
    fn instantiate_rejects_entry_returning_failure() {
        let log = Rc::new(FixtureLog::default());

        assert!(matches!(
            instance(failing_entry, &log),
            Err(AbiCallError::EntryFailed(ABI_STATUS_ERROR))
        ));
    }

    #[test]
    fn invoke_grows_output_buffer_until_plugin_fits() {
        let log = Rc::new(FixtureLog::default());
        let mut plugin = instance(fixture_entry, &log).unwrap();

        plugin.start().unwrap();
        assert_eq!(plugin.context().starts.get(), 1);
        // 插件拿到的宿主上下文必须是宿主自己绑定的那一份，不能由插件自行声明。
        assert_eq!(
            plugin.host().user_data,
            plugin.context() as *const Rc<FixtureLog> as *mut c_void
        );
        let output = plugin.invoke("demo.echo", b"{}").unwrap();
        plugin.stop().unwrap();

        assert_eq!(output, FIXTURE_PAYLOAD.to_vec());
        // 首次容量不足被拒，扩容后第二次成功。
        assert_eq!(log.invokes.get(), 2);
        assert_eq!(log.stops.get(), 1);
    }

    #[test]
    fn invoke_rejects_output_above_the_abi_limit() {
        let log = Rc::new(FixtureLog::default());
        let mut plugin = instance(greedy_entry, &log).unwrap();

        assert!(matches!(
            plugin.invoke("demo.echo", b"{}"),
            Err(AbiCallError::Validation(
                AbiValidationError::BufferTooLarge { .. }
            ))
        ));
    }

    #[test]
    fn invoke_rejects_success_with_length_above_capacity() {
        let log = Rc::new(FixtureLog::default());
        let mut plugin = instance(overreporting_entry, &log).unwrap();

        assert!(matches!(
            plugin.invoke("demo.echo", b"{}"),
            Err(AbiCallError::ContractViolation(_))
        ));
    }

    #[test]
    fn invoke_rejects_oversized_input_without_calling_plugin() {
        let log = Rc::new(FixtureLog::default());
        let mut plugin = instance(fixture_entry, &log).unwrap();
        let oversized = vec![0_u8; MAX_ABI_BUFFER_BYTES + 1];

        assert!(matches!(
            plugin.invoke("demo.echo", &oversized),
            Err(AbiCallError::Validation(
                AbiValidationError::BufferTooLarge { .. }
            ))
        ));
        assert_eq!(log.invokes.get(), 0);
    }

    #[test]
    fn destroy_is_idempotent_and_drop_releases_plugin_state() {
        let log = Rc::new(FixtureLog::default());
        {
            let mut plugin = instance(fixture_entry, &log).unwrap();
            plugin.destroy();
            assert_eq!(log.destroys.get(), 1);

            plugin.destroy();
            assert_eq!(log.destroys.get(), 1);
        }

        // drop 不应重复归还已释放的状态。
        assert_eq!(log.destroys.get(), 1);
    }
}
