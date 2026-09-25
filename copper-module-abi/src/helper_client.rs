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
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::ipc::{
    read_frame, write_frame, IpcError, IpcRequest, IpcResponse, ModuleMethod, PROTOCOL_VERSION,
};

/// stderr 只保留末尾这么多字节用于诊断，避免无界增长。
const STDERR_TAIL_BYTES: usize = 8 * 1024;
/// 等待进程退出的轮询间隔。
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

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

/// 与一个 helper 子进程的同步会话。
pub struct HelperProcess {
    child: Child,
    /// `None` 表示 stdin 已关闭（停机流程已启动）。
    stdin: Option<ChildStdin>,
    events: Receiver<HelperEvent>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    module_id: String,
    request_seq: u64,
    negotiated_version: Option<u32>,
}

impl HelperProcess {
    /// 派生 helper 进程。此时尚未握手，也尚未加载插件。
    pub fn spawn(program: &Path, module_id: &str, plugin_path: &Path) -> Result<Self, HelperError> {
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

        let stdin = child.stdin.take().ok_or(HelperError::MissingPipe)?;
        let stdout = child.stdout.take().ok_or(HelperError::MissingPipe)?;
        let stderr = child.stderr.take().ok_or(HelperError::MissingPipe)?;
        let stderr_tail = spawn_stderr_collector(stderr);

        let (sender, events) = mpsc::channel();
        std::thread::spawn(move || read_responses(stdout, sender));

        Ok(Self {
            child,
            stdin: Some(stdin),
            events,
            stderr_tail,
            module_id: module_id.to_owned(),
            request_seq: 0,
            negotiated_version: None,
        })
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

        let encoded = serde_json::to_vec(&request).map_err(IpcError::Json)?;
        let write_error = {
            let stdin = self.stdin.as_mut().ok_or(HelperError::MissingPipe)?;
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
        self.stdin.take();
        self.wait_for_exit(timeout)
    }

    /// 强制终止并回收进程。
    pub fn terminate(&mut self) -> Result<(), HelperError> {
        self.stdin.take();
        let _ = self.child.kill();
        self.child.wait().map_err(HelperError::Spawn)?;
        Ok(())
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
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 把 stdout 上的响应帧投递到通道。通道接收端消失即结束，不产生后台泄漏。
fn read_responses(stdout: ChildStdout, sender: mpsc::Sender<HelperEvent>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let payload = match read_frame(&mut reader) {
            Ok(payload) => payload,
            // EOF 或坏帧：结束线程即可，调用方会通过通道断开感知到。
            Err(_) => return,
        };
        let event = match serde_json::from_slice::<IpcResponse>(&payload) {
            Ok(response) => HelperEvent::Response(response),
            Err(error) => HelperEvent::Malformed(error.to_string()),
        };
        if sender.send(event).is_err() {
            return;
        }
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
