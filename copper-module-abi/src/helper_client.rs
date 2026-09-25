//! 宿主侧 helper 客户端：派生 helper 进程、按协议驱动它，并把故障转成显式错误。
//!
//! 这是 IPC 协议的另一端，因此与 [`crate::ipc`] 同属契约层：内核用它管理真实
//! helper，集成测试用同一实现驱动真实进程，两边不会各自演进出一套协议细节。
//!
//! # 超时与隔离
//!
//! 读响应由独立 IO 线程完成并投递到通道，调用方用 `recv_timeout` 等待。因此
//! helper 卡死不会让内核无限阻塞；helper 退出会让通道断开，调用方立刻得到
//! 带 stderr 尾部的错误，而不是永久挂起。

use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::ipc::{
    read_frame, write_frame, CapabilityRequest, IpcError, IpcErrorDto, IpcMessage, IpcRequest,
    IpcResponse, ModuleMethod, METHOD_CAPABILITY_REQUEST, PROTOCOL_VERSION,
};

/// stderr 只保留末尾这么多字节用于诊断，避免无界增长。
const STDERR_TAIL_BYTES: usize = 8 * 1024;
/// 等待进程退出的轮询间隔。
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// 插件能力请求被拒绝时返回给 helper 的结构化原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityError {
    pub code: &'static str,
    pub message: String,
}

/// 宿主能力派发器。
///
/// 实现方拿到的 `module_id` **由宿主会话绑定**，不是插件自报的：helper 只把它从
/// 启动参数里透传过来。派发器必须按该身份逐项授权，未知能力默认拒绝。
pub trait CapabilityDispatcher: Send + Sync + 'static {
    fn dispatch(
        &self,
        module_id: &str,
        request: CapabilityRequest,
    ) -> Result<Value, CapabilityError>;
}

/// 默认派发器：拒绝一切能力请求。
///
/// 用作 [`HelperProcess::spawn`] 的默认行为，使"未接入能力服务"与"显式拒绝插件"
/// 表现一致——绝不因为没接线而放行。
pub struct NoCapabilities;

impl CapabilityDispatcher for NoCapabilities {
    fn dispatch(
        &self,
        _module_id: &str,
        request: CapabilityRequest,
    ) -> Result<Value, CapabilityError> {
        Err(CapabilityError {
            code: "capability_not_supported",
            message: format!("capability `{}` is not available", request.capability),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HelperError {
    #[error("failed to spawn the helper process: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("helper process is missing a required stdio pipe")]
    MissingPipe,
    #[error("helper IPC failed: {0}")]
    Ipc(#[from] IpcError),
    #[error("helper exited before replying: {stderr}")]
    Exited { stderr: String },
    #[error("helper did not reply within {timeout_ms} ms")]
    Timeout { timeout_ms: u64 },
    #[error("helper replied to request `{received}` while `{expected}` was pending")]
    RequestIdMismatch { expected: String, received: String },
    #[error("helper rejected the request: [{code}] {message}")]
    Remote { code: String, message: String },
    #[error("helper reported success without a result payload")]
    MissingResult,
}

/// IO 线程交给调用方的消息。
enum HelperEvent {
    Response(IpcResponse),
    /// 帧能读出但无法解析成响应：协议级故障，不应被当作普通错误吞掉。
    Malformed(String),
}

/// 单向推送失败的原因。
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    /// 写端已关闭：helper 已停机或已被回收。
    #[error("helper push channel is closed")]
    Closed,
    #[error("helper push frame failed: {0}")]
    Ipc(#[from] IpcError),
}

/// 只写的推送句柄：向 helper 发**单向通知**，永不等待响应。
///
/// # 为什么必须是只写且不等待
///
/// 通知与 [`HelperProcess::request`] 共用同一个 stdin，帧在锁内整体写出，因此不会
/// 字节交错。但通知**不参与请求 / 响应配对**：helper 对通知一律不回帧。若对端回了
/// 帧，正在等待某个 invoke 响应的调用方会读到不属于它的响应，把一次成功的调用判成
/// [`HelperError::RequestIdMismatch`] 并污染后续配对。
///
/// 因此本句柄刻意**不持有响应通道**，结构上就无法等待响应。
#[derive(Clone)]
pub struct HelperPushHandle {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    /// 通知在会话内的序号，只用于日志可读性（没有响应需要与它配对）。
    seq: Arc<AtomicU64>,
    module_id: String,
}

impl HelperPushHandle {
    /// 句柄所属的模块身份（宿主下发，插件无法改写）。
    pub fn module_id(&self) -> &str {
        &self.module_id
    }

    /// 推送一条事件通知。
    ///
    /// `event.dispatch` 的载荷格式属本 crate 的协议细节，调用方不必自己拼。
    pub fn push_event(&self, event: &str, payload: &Value) -> Result<(), PushError> {
        self.notify(
            crate::ipc::METHOD_EVENT_DISPATCH,
            json!({ "event": event, "payload": payload }),
        )
    }

    /// 发送一条单向通知。失败即表示 helper 已不可用（进程退出或管道关闭）。
    pub fn notify(&self, method: &str, params: Value) -> Result<(), PushError> {
        let request = IpcRequest {
            version: PROTOCOL_VERSION,
            // `push-` 前缀与请求的 `req-` 前缀分属两个命名空间，日志里一眼可辨。
            request_id: format!("push-{}", self.seq.fetch_add(1, Ordering::Relaxed)),
            method: method.to_owned(),
            params,
        };
        let encoded = IpcMessage::Request(request).encode()?;

        let mut guard = self
            .stdin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stdin = guard.as_mut().ok_or(PushError::Closed)?;
        write_frame(stdin, &encoded).and_then(|()| stdin.flush().map_err(IpcError::Io))?;
        Ok(())
    }
}

/// 与一个 helper 子进程的同步会话。
pub struct HelperProcess {
    child: Child,
    /// 写端在两个线程间共享：主线程发方法请求，IO 线程回能力响应；
    /// [`HelperProcess::push_handle`] 也共享同一份，因此三类帧不会字节交错。
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    events: Receiver<HelperEvent>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    module_id: String,
    request_seq: u64,
    /// 通知序号，由 [`HelperProcess::push_handle`] 分出的句柄共享。
    push_seq: Arc<AtomicU64>,
    negotiated_version: Option<u32>,
}

impl HelperProcess {
    /// 派生 helper 进程，并使用默认的「拒绝一切能力」派发器。
    pub fn spawn(program: &Path, module_id: &str, plugin_path: &Path) -> Result<Self, HelperError> {
        Self::spawn_with_dispatcher(program, module_id, plugin_path, Arc::new(NoCapabilities))
    }

    /// 派生 helper 进程，并指定能力派发器。
    pub fn spawn_with_dispatcher(
        program: &Path,
        module_id: &str,
        plugin_path: &Path,
        dispatcher: Arc<dyn CapabilityDispatcher>,
    ) -> Result<Self, HelperError> {
        let mut child = Command::new(program)
            // 结构化参数：不经 shell，避免拼接与环境变量注入。
            .arg("--module-id")
            .arg(module_id)
            .arg("--plugin")
            .arg(plugin_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(HelperError::Spawn)?;

        let stdin = Arc::new(Mutex::new(Some(
            child.stdin.take().ok_or(HelperError::MissingPipe)?,
        )));
        let stdout = child.stdout.take().ok_or(HelperError::MissingPipe)?;
        let stderr = child.stderr.take().ok_or(HelperError::MissingPipe)?;
        let stderr_tail = spawn_stderr_collector(stderr);

        let (sender, events) = mpsc::channel();
        let io_stdin = Arc::clone(&stdin);
        let io_module_id = module_id.to_owned();
        std::thread::spawn(move || {
            handle_stream(stdout, sender, io_stdin, io_module_id, dispatcher)
        });

        Ok(Self {
            child,
            stdin,
            events,
            stderr_tail,
            module_id: module_id.to_owned(),
            request_seq: 0,
            push_seq: Arc::new(AtomicU64::new(0)),
            negotiated_version: None,
        })
    }

    /// 分出一个可跨线程共享的推送句柄。
    ///
    /// 事件发生在任意内核线程上，而 [`HelperProcess`] 的请求路径要求 `&mut self`
    /// 独占；推送句柄把「写通知」这条只写路径独立出来，两者共用同一把 stdin 锁。
    pub fn push_handle(&self) -> HelperPushHandle {
        HelperPushHandle {
            stdin: Arc::clone(&self.stdin),
            seq: Arc::clone(&self.push_seq),
            module_id: self.module_id.clone(),
        }
    }

    /// 会话绑定的模块身份（由宿主下发，helper 无从改写）。
    pub fn module_id(&self) -> &str {
        &self.module_id
    }

    /// 已协商的协议版本；未握手时为 `None`。
    pub fn negotiated_version(&self) -> Option<u32> {
        self.negotiated_version
    }

    /// 完成版本协商。必须成功后才能调用其它方法。
    pub fn handshake(&mut self, timeout: Duration) -> Result<u32, HelperError> {
        let result = self.request(
            ModuleMethod::Handshake,
            json!({ "supported_versions": [PROTOCOL_VERSION] }),
            timeout,
        )?;
        let version = result
            .get("version")
            .and_then(Value::as_u64)
            .ok_or(HelperError::MissingResult)?;
        let version = u32::try_from(version).map_err(|_| HelperError::MissingResult)?;
        self.negotiated_version = Some(version);
        Ok(version)
    }

    /// 请求 helper 加载并初始化插件。
    pub fn initialize(&mut self, config: &Value, timeout: Duration) -> Result<(), HelperError> {
        self.request(
            ModuleMethod::Initialize,
            json!({ "config": config }),
            timeout,
        )?;
        Ok(())
    }

    /// 启动插件。
    pub fn start(&mut self, timeout: Duration) -> Result<(), HelperError> {
        self.request(
            ModuleMethod::Start,
            json!({ "action": "start" }),
            timeout,
        )?;
        Ok(())
    }

    /// 停止插件。
    pub fn stop(&mut self, timeout: Duration) -> Result<(), HelperError> {
        self.request(ModuleMethod::Stop, json!({ "action": "stop" }), timeout)?;
        Ok(())
    }

    /// 健康检查。
    pub fn health(&mut self, timeout: Duration) -> Result<bool, HelperError> {
        let result = self.request(ModuleMethod::Health, json!({}), timeout)?;
        Ok(result
            .get("healthy")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    /// 调用插件命令，返回插件产出的 JSON。
    pub fn invoke(
        &mut self,
        command: &str,
        args: &Value,
        timeout: Duration,
    ) -> Result<Value, HelperError> {
        let result = self.request(
            ModuleMethod::Invoke,
            json!({ "command": command, "args": args }),
            timeout,
        )?;
        result
            .get("result")
            .cloned()
            .ok_or(HelperError::MissingResult)
    }

    /// 发送一条方法请求并等待其响应。
    ///
    /// 未知方法、非法参数、权限拒绝等一律以 [`HelperError::Remote`] 返回，调用方
    /// 不需要（也不应该）解析响应内部的错误 DTO 结构之外的东西。
    pub fn request(
        &mut self,
        method: ModuleMethod,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, HelperError> {
        let request_id = format!("req-{}", self.request_seq);
        self.request_seq += 1;
        let request = IpcRequest {
            version: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            method: method.as_str().to_owned(),
            params,
        };

        let encoded = IpcMessage::Request(request).encode()?;
        let write_error = {
            let mut guard = self
                .stdin
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let stdin = guard.as_mut().ok_or(HelperError::MissingPipe)?;
            write_frame(stdin, &encoded)
                .and_then(|()| stdin.flush().map_err(IpcError::Io))
                .err()
        };
        if let Some(error) = write_error {
            // 写失败通常意味着对端已经退出（例如 helper 启动自检未通过）。把它归为
            // "进程已退出"并带上 stderr，避免调用方只拿到一个没有上下文的 IO 错误。
            if self.has_exited().unwrap_or(false) {
                return Err(HelperError::Exited {
                    stderr: format!("{error}; {}", self.stderr()),
                });
            }
            return Err(HelperError::Ipc(error));
        }

        let response = match self.events.recv_timeout(timeout) {
            Ok(HelperEvent::Response(response)) => response,
            Ok(HelperEvent::Malformed(detail)) => {
                return Err(HelperError::Exited {
                    stderr: format!("helper sent a malformed frame: {detail}; {}", self.stderr()),
                })
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(HelperError::Timeout {
                    timeout_ms: timeout.as_millis() as u64,
                })
            }
            // 通道断开：helper 已退出（或 IO 线程因帧错误结束）。
            Err(RecvTimeoutError::Disconnected) => {
                return Err(HelperError::Exited {
                    stderr: self.stderr(),
                })
            }
        };

        if response.request_id != request_id {
            return Err(HelperError::RequestIdMismatch {
                expected: request_id,
                received: response.request_id,
            });
        }
        if let Some(error) = response.error {
            return Err(HelperError::Remote {
                code: error.code,
                message: error.message,
            });
        }
        response.result.ok_or(HelperError::MissingResult)
    }

    /// 优雅停机：请插件停止 → 关闭 stdin → 等待退出，超时则强制终止。
    ///
    /// 返回进程是否在超时前自行退出，便于上层区分"干净停机"与"被强杀"。
    pub fn shutdown(&mut self, timeout: Duration) -> Result<bool, HelperError> {
        // 插件停机失败不阻断回收：真正的目标是确保进程与资源被释放。
        let _ = self.stop(timeout);
        self.close_stdin();
        self.wait_for_exit(timeout)
    }

    /// 强制终止并回收进程。
    pub fn terminate(&mut self) -> Result<(), HelperError> {
        self.close_stdin();
        let _ = self.child.kill();
        self.child.wait().map_err(HelperError::Spawn)?;
        Ok(())
    }

    /// 关闭写端：helper 读到 EOF 会正常退出。
    fn close_stdin(&self) {
        let mut guard = self
            .stdin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = None;
    }

    /// 当前进程是否已退出。
    pub fn has_exited(&mut self) -> Result<bool, HelperError> {
        self.child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(HelperError::Spawn)
    }

    /// stderr 尾部内容（限长），用于诊断。
    pub fn stderr(&self) -> String {
        match self.stderr_tail.lock() {
            Ok(buffer) => String::from_utf8_lossy(&buffer).into_owned(),
            Err(poisoned) => String::from_utf8_lossy(&poisoned.into_inner()).into_owned(),
        }
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Result<bool, HelperError> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.has_exited()? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                self.child.wait().map_err(HelperError::Spawn)?;
                return Ok(false);
            }
            std::thread::sleep(EXIT_POLL_INTERVAL);
        }
    }
}

impl Drop for HelperProcess {
    fn drop(&mut self) {
        // 绝不留孤儿进程：drop 时若进程仍在，直接终止并回收。
        self.close_stdin();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 读取 helper 的帧流，并在同一个线程里回答它转来的能力请求。
///
/// 两个方向共用一个线程是必要的：能力请求必须在宿主仍在等待当前方法响应的**同时**
/// 被处理，否则会死锁。收到响应投递给调用方，收到请求就地派发。
fn handle_stream(
    stdout: ChildStdout,
    sender: mpsc::Sender<HelperEvent>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    module_id: String,
    dispatcher: Arc<dyn CapabilityDispatcher>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let payload = match read_frame(&mut reader) {
            Ok(payload) => payload,
            // EOF 或坏帧：结束线程即可，调用方会通过通道断开感知到。
            Err(_) => return,
        };

        match IpcMessage::decode(&payload) {
            Ok(IpcMessage::Response(response)) => {
                if sender.send(HelperEvent::Response(response)).is_err() {
                    return;
                }
            }
            Ok(IpcMessage::Request(request)) => {
                let response = answer_capability_request(&*dispatcher, &module_id, request);
                let encoded = match IpcMessage::Response(response).encode() {
                    Ok(encoded) => encoded,
                    Err(_) => return,
                };
                let mut guard = match stdin.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                let Some(writer) = guard.as_mut() else {
                    return;
                };
                if write_frame(writer, &encoded).is_err() || writer.flush().is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(HelperEvent::Malformed(error.to_string()));
                return;
            }
        }
    }
}

/// 把一个来自 helper 的请求转成响应：只接受能力请求，其余方法一律拒绝。
fn answer_capability_request(
    dispatcher: &dyn CapabilityDispatcher,
    module_id: &str,
    request: IpcRequest,
) -> IpcResponse {
    let version = request.version;
    let request_id = request.request_id;
    let error = |code: &str, message: String| IpcResponse {
        version,
        request_id: request_id.clone(),
        result: None,
        error: Some(IpcErrorDto {
            code: code.to_owned(),
            message,
        }),
    };

    if request.method != METHOD_CAPABILITY_REQUEST {
        return error(
            "method_not_found",
            format!("host does not accept requests with method `{}`", request.method),
        );
    }

    let capability: CapabilityRequest = match serde_json::from_value(request.params) {
        Ok(capability) => capability,
        Err(parse_error) => return error("invalid_params", parse_error.to_string()),
    };

    match dispatcher.dispatch(module_id, capability) {
        Ok(result) => IpcResponse {
            version,
            request_id,
            result: Some(result),
            error: None,
        },
        Err(denied) => error(denied.code, denied.message),
    }
}

/// 收集 helper 的 stderr，只保留末尾固定字节数。
fn spawn_stderr_collector(stderr: ChildStderr) -> Arc<Mutex<Vec<u8>>> {
    let tail = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&tail);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut chunk = [0_u8; 1024];
        loop {
            let read = match reader.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(read) => read,
            };
            let Ok(mut buffer) = sink.lock() else {
                return;
            };
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.len() > STDERR_TAIL_BYTES {
                let excess = buffer.len() - STDERR_TAIL_BYTES;
                buffer.drain(..excess);
            }
        }
    });
    tail
}
