use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use copper_module_abi::helper_client::CapabilityDispatcher;
use copper_module_abi::ipc::{
    negotiate_hello, CapabilityRequest, WireError, WireFrame, AGENT_METHODS,
    METHOD_AGENT_SHUTDOWN, METHOD_CAPABILITY_REQUEST, PROTOCOL_VERSION, WIRE_PROTOCOL_ID,
};

use crate::error::KernelError;
use crate::registry::manifest::RuntimeSpec;

const NODE_VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_NODE_VERSION_OUTPUT: usize = 64;
const MAX_NODE_INBOUND_FRAME: usize = 1024 * 1024;
const MAX_NODE_OUTBOUND_FRAME: usize = 256 * 1024;
const NODE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const NODE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PENDING_NODE_EVENTS: usize = 1024;
const MAX_PENDING_NODE_EVENT_BYTES: usize = 8 * 1024 * 1024;
const NODE_FRAME_CHANNEL_CAPACITY: usize = 16;
const MAX_NODE_STDERR_TAIL: usize = 16 * 1024;

pub(crate) struct NodeRuntimeSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    frames: std::sync::mpsc::Receiver<Result<WireFrame, String>>,
    pending_events: std::collections::VecDeque<WireFrame>,
    pending_event_bytes: usize,
    stderr_tail: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<u8>>>,
    sequence: u64,
    negotiated_version: Option<u32>,
    /// 本会话反向能力请求的宿主派发器；每模块一份，随会话绑定。
    capabilities: Arc<dyn CapabilityDispatcher>,
    /// 会话绑定的模块身份：反向能力请求的授权身份**只**取自这里，绝不读请求参数。
    module_id: String,
}

/// 宿主发给受监管 Node Agent 的首帧。
///
/// 抽成独立函数是为了让单测能直接对它的 golden line 断言：这串字面量是 Rust 与
/// TypeScript 共用的线格式契约，任何字段顺序或取值变动都必须立刻显形。
pub(crate) fn host_hello_frame() -> WireFrame {
    WireFrame::Hello {
        version: PROTOCOL_VERSION,
        protocol: WIRE_PROTOCOL_ID.to_owned(),
        supported_versions: vec![PROTOCOL_VERSION],
        runtime: "copper-core".to_owned(),
        capabilities: AGENT_METHODS.iter().map(|method| (*method).to_owned()).collect(),
    }
}

impl NodeRuntimeSession {
    pub(crate) fn launch(
        executable: &Path,
        entry: &Path,
        capabilities: Arc<dyn CapabilityDispatcher>,
        module_id: &str,
    ) -> Result<Self, KernelError> {
        validate_runtime_platform()?;
        let mut child = Command::new(executable)
            .arg(entry)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| KernelError::Module(format!("启动 Node runtime 失败: {error}")))?;
        let stdin = child.stdin.take().ok_or_else(|| KernelError::Module("Node runtime 缺少 stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| KernelError::Module("Node runtime 缺少 stdout".into()))?;
        let stderr_tail = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
        if let Some(stderr) = child.stderr.take() {
            let tail = std::sync::Arc::clone(&stderr_tail);
            std::thread::spawn(move || drain_stderr(stderr, tail));
        }
        let (sender, frames) = std::sync::mpsc::sync_channel(NODE_FRAME_CHANNEL_CAPACITY);
        std::thread::spawn(move || read_node_frames(stdout, sender));
        let mut session = Self {
            child,
            stdin,
            frames,
            pending_events: std::collections::VecDeque::new(),
            pending_event_bytes: 0,
            stderr_tail,
            sequence: 0,
            negotiated_version: None,
            capabilities,
            module_id: module_id.to_owned(),
        };
        if let Err(error) = session.handshake() {
            let _ = session.terminate();
            return Err(error);
        }
        Ok(session)
    }

    /// 协商统一 v2 信封。
    ///
    /// 首帧必须是对方自己的 hello；协商前收到任何业务帧都属于协议错误，这里直接
    /// 终止会话而不是猜测性兼容——旧版私有帧已被整体移除，回退解释只会掩盖协议错位。
    fn handshake(&mut self) -> Result<(), KernelError> {
        self.write_frame(&host_hello_frame())?;
        let deadline = std::time::Instant::now() + NODE_HANDSHAKE_TIMEOUT;
        loop {
            let frame = self.receive_until(deadline)?;
            match frame {
                WireFrame::Hello { .. } => {
                    let version = negotiate_hello(&frame).map_err(|error| {
                        KernelError::Module(format!("Node runtime 协议协商失败: {error}"))
                    })?;
                    self.negotiated_version = Some(version);
                    return Ok(());
                }
                WireFrame::Fatal { error, .. } => {
                    return Err(KernelError::Module(format!(
                        "Node runtime 握手致命错误 {}: {}",
                        error.code, error.message
                    )));
                }
                other => {
                    return Err(KernelError::Module(format!(
                        "Node runtime 握手期间收到非 hello 帧: {other:?}"
                    )));
                }
            }
        }
    }

    pub fn negotiated_version(&self) -> Option<u32> {
        self.negotiated_version
    }

    pub(crate) fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, KernelError> {
        let id = self.next_id();
        self.write_frame(&WireFrame::request(id.clone(), method, params))?;
        let deadline = std::time::Instant::now() + NODE_REQUEST_TIMEOUT;
        loop {
            let frame = self.receive_until(deadline)?;
            match frame {
                WireFrame::Event { .. } => {
                    // 事件是单向流式输出，不占请求关联：先入队，继续等本次响应。
                    let event_size = serde_json::to_vec(&frame).map(|encoded| encoded.len()).unwrap_or(MAX_NODE_INBOUND_FRAME);
                    if self.pending_events.len() >= MAX_PENDING_NODE_EVENTS
                        || self.pending_event_bytes.saturating_add(event_size) > MAX_PENDING_NODE_EVENT_BYTES
                    {
                        return Err(KernelError::Module("Node runtime 待处理事件队列超限".into()));
                    }
                    self.pending_event_bytes += event_size;
                    self.pending_events.push_back(frame);
                }
                WireFrame::Response { id: response_id, result, error, .. } => {
                    if response_id != id {
                        return Err(KernelError::Module("Node runtime 响应 id 不匹配".into()));
                    }
                    if let Some(error) = error {
                        return Err(KernelError::Module(format!(
                            "Node runtime 请求失败 {}: {}",
                            error.code, error.message
                        )));
                    }
                    return result.ok_or_else(|| {
                        KernelError::Module("Node runtime 响应缺少 result".into())
                    });
                }
                WireFrame::Fatal { error, .. } => {
                    return Err(KernelError::Module(format!(
                        "Node runtime 致命错误 {}: {}",
                        error.code, error.message
                    )));
                }
                // Agent 反向请求宿主能力：就地派发并回帧，然后继续等本次请求自己的响应。
                // 无论派发成功还是被拒，都**不得**终止会话——能力请求是 Agent 正常工作
                // 的一部分，拒绝只该体现为一次结构化错误响应。
                WireFrame::Request { id: request_id, method, params, .. } => {
                    self.answer_inbound_request(&request_id, &method, params)?;
                }
                WireFrame::Notification { method, .. } => {
                    return Err(KernelError::Module(format!(
                        "Node runtime 发送了通知（{method}）但宿主侧派发尚未实现"
                    )));
                }
                WireFrame::Hello { .. } => {
                    return Err(KernelError::Module("Node runtime 重复握手".into()));
                }
            }
        }
    }

    /// 把一个来自 Agent 的反向请求就地转成响应帧。
    ///
    /// 授权身份只取自会话绑定的 `module_id`：请求参数里的任何身份字段都会被
    /// [`CapabilityRequest`] 的反序列化直接拒绝，宿主也从不读它。
    fn answer_capability_request(
        &self,
        request_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> WireFrame {
        let error = |code: &str, message: String| WireFrame::Response {
            version: PROTOCOL_VERSION,
            id: request_id.to_owned(),
            result: None,
            error: Some(WireError {
                code: code.to_owned(),
                message,
            }),
        };

        if method != METHOD_CAPABILITY_REQUEST {
            return error(
                "method_not_found",
                format!("宿主不接受方法 `{method}` 的请求"),
            );
        }

        let capability: CapabilityRequest = match serde_json::from_value(params) {
            Ok(capability) => capability,
            Err(parse_error) => return error("bad_frame", parse_error.to_string()),
        };

        match self.capabilities.dispatch(&self.module_id, capability) {
            Ok(result) => WireFrame::Response {
                version: PROTOCOL_VERSION,
                id: request_id.to_owned(),
                result: Some(result),
                error: None,
            },
            Err(denied) => error(denied.code, denied.message),
        }
    }

    fn take_event(&mut self) -> Option<WireFrame> {
        let event = self.pending_events.pop_front()?;
        self.pending_event_bytes = self.pending_event_bytes.saturating_sub(
            serde_json::to_vec(&event).map(|encoded| encoded.len()).unwrap_or(MAX_NODE_INBOUND_FRAME),
        );
        Some(event)
    }

    /// 应答一条 Agent 主动发来的反向请求：就地派发并把响应帧写回。
    ///
    /// [`NodeRuntimeSession::request`] 与 [`NodeRuntimeSession::pump_events`] 共用这一条
    /// 路径。两处各写一份迟早会漂移，而「反向请求必须被应答、且无论派发结果如何都不得
    /// 终止会话」是会话的核心不变量，只允许有一个实现。
    fn answer_inbound_request(
        &mut self,
        request_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), KernelError> {
        let response = self.answer_capability_request(request_id, method, params);
        self.write_frame(&response)
    }

    /// 主动把 Agent 发来的事件泵出来，逐条交给 `on_event`。
    ///
    /// 这是「事件上行」的唯一出口：`request` 只在等响应的间隙**被动**缓冲事件，而长会话
    /// 里 Agent 会在两次请求之间持续产出流式事件。必须有一条主动拉取的路径，才能把它们
    /// 送到内核事件总线（进而桥接前端）。
    ///
    /// 顺序语义：先排空 `request` 期间缓冲的事件，再读新帧。若不这样，新帧会插到更早的
    /// 缓冲事件之前——同一条流的两段被重排，前端看到的 `sequence` 就乱了。
    ///
    /// 返回本次实际投递（即回调）的事件条数，便于调用方记账与测试断言。
    pub(crate) fn pump_events(
        &mut self,
        idle_timeout: Duration,
        max_frames: usize,
        on_event: &mut dyn FnMut(&WireFrame),
    ) -> Result<usize, KernelError> {
        let mut delivered = 0usize;

        // 第一段：排空等待响应期间被缓冲的事件，数量口径与 `take_event` 完全一致。
        while delivered < max_frames {
            let Some(event) = self.take_event() else { break };
            on_event(&event);
            delivered += 1;
        }
        // 缓冲没被排空说明本轮预算已用尽：必须立刻返回，否则第二段读到的**更新**帧会越过
        // 仍在缓冲里的**更早**事件先被投递，顺序语义被破坏。
        if !self.pending_events.is_empty() {
            return Ok(delivered);
        }

        // 第二段：主动读新帧，预算同为 `max_frames`。空闲满 `idle_timeout` 即正常收尾，
        // 把会话锁让出去给转发命令（泵与 `request` 互斥同一把锁）。
        for _ in 0..max_frames {
            let frame = match self.frames.recv_timeout(idle_timeout) {
                Ok(Ok(frame)) => frame,
                Ok(Err(error)) => {
                    return Err(KernelError::Module(format!("Node runtime NDJSON 无效: {error}")));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(KernelError::Module("Node runtime 已退出".into()));
                }
            };
            match frame {
                WireFrame::Event { .. } => {
                    on_event(&frame);
                    delivered += 1;
                }
                // 与 `request` 同一套处理：反向能力请求就地应答后继续泵，绝不因此终止会话。
                WireFrame::Request { id, method, params, .. } => {
                    self.answer_inbound_request(&id, &method, params)?;
                }
                WireFrame::Response { id, .. } => {
                    // 泵里没有任何在等的请求：此时收到响应帧只能说明两端对请求关联的理解
                    // 已经错位。继续跑只会让状态越走越偏，如实报错。
                    return Err(KernelError::Module(format!("收到无人等待的响应帧（id `{id}`）")));
                }
                WireFrame::Fatal { error, .. } => {
                    return Err(KernelError::Module(format!(
                        "Node runtime 致命错误 {}: {}",
                        error.code, error.message
                    )));
                }
                WireFrame::Hello { .. } => {
                    return Err(KernelError::Module("Node runtime 重复握手".into()));
                }
                WireFrame::Notification { method, .. } => {
                    return Err(KernelError::Module(format!(
                        "Node runtime 发送了通知（{method}）但宿主侧派发尚未实现"
                    )));
                }
            }
        }
        Ok(delivered)
    }

    fn stderr_tail(&self) -> String {
        let bytes = self.stderr_tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let bytes = bytes.iter().copied().collect::<Vec<_>>();
        redact_node_stderr(&String::from_utf8_lossy(&bytes))
    }

    pub(crate) fn shutdown(&mut self, timeout: Duration) -> Result<(), KernelError> {
        let shutdown = self.request(METHOD_AGENT_SHUTDOWN, serde_json::json!({}));
        if shutdown.is_err() {
            return self.terminate();
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.child.try_wait().map_err(|error| KernelError::Module(format!("等待 Node runtime 失败: {error}")))?.is_some() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return self.terminate();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn terminate(&mut self) -> Result<(), KernelError> {
        let _ = self.child.kill();
        self.child.wait().map_err(|error| KernelError::Module(format!("回收 Node runtime 失败: {error}")))?;
        Ok(())
    }

    fn next_id(&mut self) -> String {
        let id = format!("host-{}", self.sequence);
        self.sequence += 1;
        id
    }

    fn write_frame(&mut self, frame: &WireFrame) -> Result<(), KernelError> {
        use std::io::Write;
        // encode_line 已自带 8 MiB 协议硬上限校验，并保证输出以换行结尾；
        // 这里只再叠加 runtime 自己的更严格出站上限。
        let encoded = frame
            .encode_line()
            .map_err(|error| KernelError::Module(format!("Node runtime 出站帧无效: {error}")))?;
        if encoded.len() > MAX_NODE_OUTBOUND_FRAME {
            return Err(KernelError::Module("Node runtime 出站帧超过 256 KiB 限制".into()));
        }
        self.stdin.write_all(&encoded).and_then(|()| self.stdin.flush())
            .map_err(|error| KernelError::Module(format!("写入 Node runtime 失败: {error}")))
    }

    fn receive_until(&mut self, deadline: std::time::Instant) -> Result<WireFrame, KernelError> {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match self.frames.recv_timeout(remaining) {
            Ok(Ok(frame)) => Ok(frame),
            Ok(Err(error)) => Err(KernelError::Module(format!("Node runtime NDJSON 无效: {error}"))),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(KernelError::Module("等待 Node runtime 响应超时".into())),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(KernelError::Module("Node runtime 已退出".into())),
        }
    }
}

impl Drop for NodeRuntimeSession {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

fn read_node_frames(mut reader: impl std::io::Read, sender: std::sync::mpsc::SyncSender<Result<WireFrame, String>>) {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(&mut reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        let mut overflow = false;
        loop {
            let available = match reader.fill_buf() {
                Ok(buffer) if buffer.is_empty() => {
                    if line.is_empty() {
                        return;
                    }
                    let _ = sender.send(Err("NDJSON stream ended with an unterminated frame".into()));
                    return;
                }
                Ok(buffer) => buffer,
                Err(error) => { let _ = sender.send(Err(error.to_string())); return; }
            };
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(available.len(), |index| index + 1);
            let content_length = count - usize::from(newline.is_some());
            if !overflow {
                if line.len() + content_length > MAX_NODE_INBOUND_FRAME {
                    overflow = true;
                    line.clear();
                } else {
                    line.extend_from_slice(&available[..content_length]);
                }
            }
            reader.consume(count);
            if newline.is_some() { break; }
        }
        if overflow {
            let _ = sender.send(Err("入站帧超过 1 MiB 限制".into()));
            return;
        }
        // 保留既有的行尾处理：\r 由这里剥掉，行内的 \r / \n 交由 decode_line 判非法。
        if line.last() == Some(&b'\r') { line.pop(); }
        if line.is_empty() { continue; }
        // 严格走统一 v2 信封解码：这里不再做任何私有帧的猜测性兼容。
        match WireFrame::decode_line(&line) {
            Ok(frame) => if sender.send(Ok(frame)).is_err() { return; },
            Err(error) => {
                let _ = sender.send(Err(format!("帧不符合 copper-addon.ndjson v2 契约: {error}")));
                return;
            }
        }
    }
}

fn drain_stderr(
    mut reader: impl std::io::Read,
    tail: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<u8>>>,
) {
    let mut buffer = [0u8; 2048];
    while let Ok(count) = reader.read(&mut buffer) {
        if count == 0 {
            return;
        }
        let mut tail = tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for byte in &buffer[..count] {
            if tail.len() == MAX_NODE_STDERR_TAIL {
                tail.pop_front();
            }
            tail.push_back(*byte);
        }
    }
}

fn redact_node_stderr(input: &str) -> String {
    let mut output = input.to_owned();
    let patterns = ["authorization", "api_key", "api-key", "bearer"];
    let mut cursor = 0;
    while cursor < output.len() {
        let lower = output[cursor..].to_ascii_lowercase();
        let Some((relative, pattern)) = patterns
            .iter()
            .filter_map(|pattern| lower.find(pattern).map(|position| (position, *pattern)))
            .min_by_key(|(position, _)| *position)
        else {
            break;
        };
        let start = cursor + relative;
        let suffix_start = start + pattern.len();
        let mut value_start = suffix_start;
        if pattern == "bearer" {
            while output[value_start..].starts_with([' ', '\t']) {
                value_start += 1;
            }
        } else {
            let Some(separator) = output[value_start..].find(['=', ':']) else {
                cursor = suffix_start;
                continue;
            };
            value_start += separator + 1;
            while output[value_start..].starts_with([' ', '\t']) {
                value_start += 1;
            }
            if output[value_start..].to_ascii_lowercase().starts_with("bearer ") {
                value_start += "bearer ".len();
            }
        }
        let value_end = output[value_start..]
            .find(|character: char| character.is_whitespace() || matches!(character, '"' | '\'' | ',' | ';'))
            .map(|offset| value_start + offset)
            .unwrap_or(output.len());
        if value_start == value_end {
            cursor = suffix_start;
            continue;
        }
        output.replace_range(value_start..value_end, "[redacted]");
        cursor = value_start + "[redacted]".len();
    }
    output
}

pub fn resolve_node_executable(
    explicit: Option<&Path>,
    search_path: &OsStr,
    windows: bool,
) -> Result<PathBuf, KernelError> {
    if let Some(path) = explicit {
        return path.is_file().then(|| path.to_path_buf()).ok_or_else(|| {
            KernelError::Module(format!("配置的 Node 可执行文件不存在: {}", path.display()))
        });
    }

    let executable = if windows { "node.exe" } else { "node" };
    std::env::split_paths(search_path)
        .map(|directory| directory.join(executable))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| KernelError::Module("PATH 中未找到 Node 可执行文件".into()))
}

pub fn parse_node_version(output: &str) -> Result<semver::Version, KernelError> {
    let value = output.trim().strip_prefix('v').unwrap_or(output.trim());
    semver::Version::parse(value).map_err(|error| {
        KernelError::Module(format!("Node --version 输出无效 `{value}`: {error}"))
    })
}

pub fn validate_node_version(
    output: &str,
    requirement: &semver::VersionReq,
) -> Result<semver::Version, KernelError> {
    let version = parse_node_version(output)?;
    if version < semver::Version::new(22, 19, 0) {
        return Err(KernelError::Module(format!(
            "Node 版本必须 ≥22.19.0，实际为 {version}"
        )));
    }
    if !requirement.matches(&version) {
        return Err(KernelError::Module(format!(
            "Node 版本 {version} 不满足模块要求 `{requirement}`"
        )));
    }
    Ok(version)
}

pub fn validate_runtime_platform() -> Result<(), KernelError> {
    if cfg!(target_os = "android") {
        Err(KernelError::Module(
            "Android 不支持受监管 Node runtime".into(),
        ))
    } else {
        Ok(())
    }
}

pub fn resolve_runtime_entry(module_dir: &Path, runtime: &RuntimeSpec) -> Result<PathBuf, KernelError> {
    let entry = runtime.resolve_entry(module_dir)?;
    let module_root = std::fs::canonicalize(module_dir)
        .map_err(|error| KernelError::Module(format!("解析模块目录失败: {error}")))?;
    let real_entry = std::fs::canonicalize(&entry)
        .map_err(|error| KernelError::Module(format!("解析 runtime 入口失败: {error}")))?;
    if !real_entry.is_file() || !real_entry.starts_with(&module_root) {
        return Err(KernelError::Module(format!(
            "runtime 入口不在模块目录内或不是文件: {}",
            entry.display()
        )));
    }
    Ok(real_entry)
}

fn read_bounded_version_output(
    mut reader: impl std::io::Read,
) -> Result<(Vec<u8>, bool), KernelError> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 256];
    let mut overflow = false;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| KernelError::Module(format!("读取 Node 版本失败: {error}")))?;
        if count == 0 {
            break;
        }
        let remaining = MAX_NODE_VERSION_OUTPUT.saturating_sub(output.len());
        let retained = count.min(remaining);
        output.extend_from_slice(&buffer[..retained]);
        overflow |= retained < count;
    }
    Ok((output, overflow))
}

pub fn inspect_node(
    executable: &Path,
    runtime: &RuntimeSpec,
) -> Result<semver::Version, KernelError> {
    validate_runtime_platform()?;
    runtime.validate()?;
    let requirement = semver::VersionReq::parse(&runtime.engines.node).map_err(|error| {
        KernelError::Module(format!("Node 版本要求无效: {error}"))
    })?;
    let mut child = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| KernelError::Module(format!("启动 Node 版本探测失败: {error}")))?;
    let stdout = child.stdout.take().ok_or_else(|| KernelError::Module("Node 版本探测缺少 stdout".into()))?;
    let reader = std::thread::spawn(move || read_bounded_version_output(stdout));
    let deadline = std::time::Instant::now() + NODE_VERSION_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| KernelError::Module(format!("检查 Node 版本进程失败: {error}")))?
        {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(KernelError::Module("Node --version 探测超时".into()));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (version_bytes, overflow) = reader
        .join()
        .map_err(|_| KernelError::Module("读取 Node 版本线程异常退出".into()))??;
    if !status.success() {
        return Err(KernelError::Module(format!("Node --version 返回失败状态: {status}")));
    }
    if overflow {
        return Err(KernelError::Module("Node --version 输出超过长度上限".into()));
    }
    let version_output = String::from_utf8(version_bytes)
        .map_err(|error| KernelError::Module(format!("Node 版本输出不是 UTF-8: {error}")))?;
    validate_node_version(&version_output, &requirement)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use copper_module_abi::helper_client::{CapabilityDispatcher, CapabilityError, NoCapabilities};
    use copper_module_abi::ipc::{CapabilityRequest, METHOD_AGENT_PING, METHOD_AGENT_PROMPT, WireFrame};

    use super::{
        inspect_node, parse_node_version, redact_node_stderr, resolve_node_executable,
        resolve_runtime_entry, validate_node_version, validate_runtime_platform,
    };
    use crate::registry::manifest::{RuntimeEngines, RuntimeSpec, WireProtocolSpec, WIRE_PROTOCOL_ID, WIRE_PROTOCOL_VERSION};

    /// 测试用的合法协议声明：声明了进程 runtime 的清单必须带它。
    fn test_wire_protocol() -> WireProtocolSpec {
        WireProtocolSpec {
            id: WIRE_PROTOCOL_ID.into(),
            version: WIRE_PROTOCOL_VERSION,
        }
    }

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("copper-node-runtime-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explicit_node_path_is_selected_without_path_fallback() {
        let temp = TempDir::new();
        let explicit = temp.0.join("node-custom.exe");
        fs::write(&explicit, b"").unwrap();
        let result = resolve_node_executable(Some(&explicit), OsStr::new("other"), true);
        assert_eq!(result.unwrap(), explicit);
    }

    #[test]
    fn configured_path_must_exist_instead_of_falling_back() {
        let temp = TempDir::new();
        let result = resolve_node_executable(
            Some(&temp.0.join("missing-node.exe")),
            temp.0.as_os_str(),
            true,
        );
        assert!(result.is_err());
    }

    #[test]
    fn path_search_selects_node_executable() {
        let temp = TempDir::new();
        let executable = temp.0.join(if cfg!(windows) { "node.exe" } else { "node" });
        fs::write(&executable, b"").unwrap();
        let search_path = std::env::join_paths([&temp.0]).unwrap();
        assert_eq!(
            resolve_node_executable(None, &search_path, cfg!(windows)).unwrap(),
            executable
        );
    }

    #[test]
    fn version_output_reader_caps_retained_bytes_while_draining_input() {
        let source = vec![b'x'; super::MAX_NODE_VERSION_OUTPUT + 16];
        let (bytes, overflow) = super::read_bounded_version_output(source.as_slice()).unwrap();
        assert!(overflow);
        assert_eq!(bytes.len(), super::MAX_NODE_VERSION_OUTPUT);
    }

    #[test]
    fn node_version_output_accepts_v_prefix_and_rejects_noise() {
        assert_eq!(parse_node_version("v22.19.1\n").unwrap().to_string(), "22.19.1");
        assert!(parse_node_version("node v22.19.1").is_err());
    }

    #[test]
    fn node_version_must_satisfy_manifest_requirement_and_minimum() {
        let req = semver::VersionReq::parse(">=22.19.0").unwrap();
        assert!(validate_node_version("v22.19.0", &req).is_ok());
        assert!(validate_node_version("v22.18.9", &req).is_err());
        assert!(validate_node_version("v23.0.0", &req).is_ok());
        let narrow_req = semver::VersionReq::parse("<23.0.0").unwrap();
        assert!(validate_node_version("v23.0.0", &narrow_req).is_err());
    }

    #[test]
    fn android_runtime_is_explicitly_rejected() {
        let result = validate_runtime_platform();
        if cfg!(target_os = "android") {
            assert!(result.is_err());
        } else {
            assert!(result.is_ok());
        }
    }

    #[test]
    fn runtime_entry_must_exist_inside_module_directory() {
        let temp = TempDir::new();
        let module_dir = temp.0.join("module");
        let runtime_dir = module_dir.join("runtime");
        fs::create_dir_all(&runtime_dir).unwrap();
        let entry = runtime_dir.join("agent.mjs");
        fs::write(&entry, b"process.exit(0)").unwrap();
        let runtime = RuntimeSpec {
            kind: "node".into(),
            entry: "runtime/agent.mjs".into(),
            engines: RuntimeEngines { node: ">=22.19.0".into() },
            wire_protocol: Some(test_wire_protocol()),
        };

        assert_eq!(resolve_runtime_entry(&module_dir, &runtime).unwrap(), fs::canonicalize(entry).unwrap());
        assert!(resolve_runtime_entry(&module_dir, &RuntimeSpec { entry: "runtime/missing.mjs".into(), ..runtime }).is_err());
    }

    #[test]
    fn inspect_node_executes_version_probe_and_checks_manifest() {
        let search_path = std::env::var_os("PATH").unwrap_or_default();
        let executable = resolve_node_executable(None, &search_path, cfg!(windows))
            .expect("Node must be installed to run the runtime integration test");
        let runtime = RuntimeSpec {
            kind: "node".into(),
            entry: "runtime/pi-session.mjs".into(),
            engines: RuntimeEngines { node: ">=22.19.0".into() },
            wire_protocol: Some(test_wire_protocol()),
        };
        let version = inspect_node(&executable, &runtime).unwrap();
        assert!(version >= semver::Version::new(22, 19, 0));
    }

    #[test]
    fn node_stderr_redacts_authorization_bearer_and_api_keys() {
        let redacted = redact_node_stderr(
            "Authorization: Bearer abc.def\napi_key=secret-value\napi-key: another-secret\nBearer standalone-token",
        );
        assert!(!redacted.contains("abc.def"));
        assert!(!redacted.contains("secret-value"));
        assert!(!redacted.contains("another-secret"));
        assert!(!redacted.contains("standalone-token"));
    }

    #[test]
    fn node_frame_reader_rejects_unterminated_final_frame() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        super::read_node_frames(
            br#"{"kind":"hello","version":2,"protocol":"copper-addon.ndjson","supported_versions":[2],"runtime":"copper-module-helper","capabilities":[]}"#.as_slice(),
            sender,
        );
        assert!(matches!(receiver.recv(), Ok(Err(_))));
    }

    #[test]
    fn node_frame_reader_rejects_frames_that_are_not_v2() {
        // 迁移到统一 v2 信封后，入站帧不得再被旧版 Agent 私有 type 帧"回退解释"，
        // 也不接受任何非 v2 版本；两者都必须直接判为错误帧，而不是各演各的。
        let v1 = concat!(
            r#"{"kind":"hello","version":1,"protocol":"copper-addon.ndjson","supported_versions":[1],"runtime":"legacy-agent","capabilities":[]}"#,
            "\n"
        );
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        super::read_node_frames(v1.as_bytes(), sender);
        assert!(matches!(receiver.recv(), Ok(Err(_))));

        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        super::read_node_frames(b"{\"type\":\"ready\"}\n".as_slice(), sender);
        assert!(matches!(receiver.recv(), Ok(Err(_))));
    }

    #[test]
    fn host_hello_frame_matches_the_shared_golden_line() {
        // 字段顺序即线格式：TypeScript 侧对同一字面量断言，任一端改动都会同时失败。
        let encoded = super::host_hello_frame().encode_line().unwrap();
        let line = std::str::from_utf8(&encoded).unwrap();
        assert!(
            line.starts_with(
                r#"{"kind":"hello","version":2,"protocol":"copper-addon.ndjson","supported_versions":[2],"runtime":"copper-core","capabilities":["#
            ),
            "host hello 前缀不匹配 shared golden frame，实际: {line}"
        );
        assert!(line.ends_with("]}\n"), "host hello 必须以换行结尾的 capabilities 数组收束，实际: {line}");
    }

    #[test]
    fn node_runtime_session_handshakes_and_roundtrips_ping() {
        let temp = TempDir::new();
        let entry = temp.0.join("session.mjs");
        fs::write(
            &entry,
            r#"import readline from 'node:readline';
const input = readline.createInterface({ input: process.stdin });
input.on('line', (line) => {
  const frame = JSON.parse(line);
  if (frame.kind === 'hello') {
    process.stdout.write(JSON.stringify({ kind: 'hello', version: 2, protocol: 'copper-addon.ndjson', supported_versions: [2], runtime: 'test-agent', capabilities: [] }) + '\n');
  } else if (frame.method === 'agent.ping') {
    process.stdout.write(JSON.stringify({ kind: 'response', version: 2, id: frame.id, result: { pong: true } }) + '\n');
  } else if (frame.method === 'agent.shutdown') {
    process.stdout.write(JSON.stringify({ kind: 'response', version: 2, id: frame.id, result: {} }) + '\n');
    process.exit(0);
  }
});"#,
        )
        .unwrap();
        let executable = resolve_node_executable(
            None,
            &std::env::var_os("PATH").unwrap_or_default(),
            cfg!(windows),
        )
        .expect("Node must be installed to run the runtime integration test");

        let mut session = super::NodeRuntimeSession::launch(
            &executable,
            &entry,
            Arc::new(NoCapabilities),
            "copper-lamp.agent",
        )
        .unwrap();
        assert_eq!(session.negotiated_version(), Some(2));
        let response = session
            .request(copper_module_abi::ipc::METHOD_AGENT_PING, serde_json::json!({}))
            .unwrap();
        assert_eq!(response["pong"], true);
        session.shutdown(Duration::from_secs(2)).unwrap();
    }

    /// 假 Agent：收到 `agent.ping` 时先向宿主发起一次 `capability.request`，把宿主
    /// 回帧里的 result / error 原样带进 ping 结果；同时实现 `agent.shutdown`。
    ///
    /// 这个脚本是验证「反向请求由宿主应答且不终止会话」的唯一手段：只有真实子进程
    /// 才会在不该回帧时真的卡住。
    const REVERSE_CAPABILITY_AGENT: &str = r#"import readline from 'node:readline';
const input = readline.createInterface({ input: process.stdin });
let pendingPing = null;
input.on('line', (line) => {
  const frame = JSON.parse(line);
  if (frame.kind === 'hello') {
    process.stdout.write(JSON.stringify({ kind: 'hello', version: 2, protocol: 'copper-addon.ndjson', supported_versions: [2], runtime: 'test-agent', capabilities: [] }) + '\n');
  } else if (frame.method === 'agent.ping') {
    pendingPing = frame.id;
    process.stdout.write(JSON.stringify({ kind: 'request', version: 2, id: 'cap-1', method: 'capability.request', params: { capability: 'module.info', params: {} } }) + '\n');
  } else if (frame.kind === 'response' && frame.id === 'cap-1') {
    process.stdout.write(JSON.stringify({ kind: 'response', version: 2, id: pendingPing, result: { pong: true, capability: frame.result ?? null, capabilityError: frame.error ?? null } }) + '\n');
  } else if (frame.method === 'agent.shutdown') {
    process.stdout.write(JSON.stringify({ kind: 'response', version: 2, id: frame.id, result: {} }) + '\n');
    process.exit(0);
  }
});"#;

    /// 记录派发身份的固定值派发器。
    struct RecordingDispatcher {
        seen: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl CapabilityDispatcher for RecordingDispatcher {
        fn dispatch(
            &self,
            module_id: &str,
            _request: CapabilityRequest,
        ) -> Result<serde_json::Value, CapabilityError> {
            self.seen.lock().unwrap().push(module_id.to_owned());
            Ok(serde_json::json!({ "stub": 42 }))
        }
    }

    fn node_executable() -> PathBuf {
        resolve_node_executable(
            None,
            &std::env::var_os("PATH").unwrap_or_default(),
            cfg!(windows),
        )
        .expect("本用例要求本机装有 Node")
    }

    /// 拉一轮事件泵，把投递到的事件按 `sequence` 记进 `seen`、并累加回调次数。
    ///
    /// 抽成函数而不是就地闭包：闭包对 `seen` 的独占借用会一直持续到调用结束，
    /// 夹在两次调用之间的断言就没法读 `seen`。每次调用现造一个短命闭包即可。
    fn pump_into(
        session: &mut super::NodeRuntimeSession,
        budget: usize,
        seen: &mut Vec<u64>,
        callbacks: &mut usize,
    ) -> usize {
        session
            .pump_events(Duration::from_millis(100), budget, &mut |frame| {
                if let WireFrame::Event { payload, .. } = frame {
                    *callbacks += 1;
                    seen.push(payload["sequence"].as_u64().unwrap());
                }
            })
            .unwrap()
    }

    #[test]
    fn a_reverse_capability_request_is_denied_without_killing_the_session() {
        let temp = TempDir::new();
        let entry = temp.0.join("agent.mjs");
        fs::write(&entry, REVERSE_CAPABILITY_AGENT).unwrap();

        let mut session = super::NodeRuntimeSession::launch(
            &node_executable(),
            &entry,
            Arc::new(NoCapabilities),
            "copper-lamp.agent",
        )
        .unwrap();

        let response = session.request(METHOD_AGENT_PING, serde_json::json!({})).unwrap();
        assert_eq!(
            response["capabilityError"]["code"],
            serde_json::json!("capability_not_supported"),
            "默认拒绝的派发器必须回带结构化错误响应"
        );

        // 关键性质：一次被拒的能力请求**不得**终止会话。
        let again = session.request(METHOD_AGENT_PING, serde_json::json!({})).unwrap();
        assert_eq!(again["pong"], serde_json::json!(true), "会话必须仍然可用");

        session.shutdown(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn a_reverse_capability_request_is_dispatched_with_the_session_bound_identity() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let dispatcher = Arc::new(RecordingDispatcher {
            seen: Arc::clone(&seen),
        });

        let temp = TempDir::new();
        let entry = temp.0.join("agent.mjs");
        fs::write(&entry, REVERSE_CAPABILITY_AGENT).unwrap();

        let mut session =
            super::NodeRuntimeSession::launch(&node_executable(), &entry, dispatcher, "copper-lamp.agent")
                .unwrap();

        let response = session.request(METHOD_AGENT_PING, serde_json::json!({})).unwrap();
        assert_eq!(
            response["capability"]["stub"],
            serde_json::json!(42),
            "派发器返回的固定值必须原样经响应帧回到 Agent"
        );
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["copper-lamp.agent".to_string()],
            "派发身份必须来自会话绑定，而非请求内容"
        );

        session.shutdown(Duration::from_secs(2)).unwrap();
    }

    /// 假 Agent：收到 `agent.prompt` 时**先**接连发 5 条 `agent.run` 事件，**再**回
    /// `response`。
    ///
    /// 这个顺序专门用来验证「等待响应期间被缓冲的事件」。因为 ack 在最后，宿主读到的
    /// 前 5 帧全是事件，它们只能被 [`NodeRuntimeSession::request`] 缓冲进 `pending_events`。
    const EVENTS_BEFORE_ACK_AGENT: &str = r#"import readline from 'node:readline';
const input = readline.createInterface({ input: process.stdin });
const emit = (event, payload) => process.stdout.write(JSON.stringify({ kind: 'event', version: 2, event, payload }) + '\n');
input.on('line', (line) => {
  const frame = JSON.parse(line);
  if (frame.kind === 'hello') {
    process.stdout.write(JSON.stringify({ kind: 'hello', version: 2, protocol: 'copper-addon.ndjson', supported_versions: [2], runtime: 'test-agent', capabilities: [] }) + '\n');
  } else if (frame.method === 'agent.prompt') {
    for (let i = 0; i < 5; i++) emit('agent.run', { runId: 'r1', sequence: i });
    process.stdout.write(JSON.stringify({ kind: 'response', version: 2, id: frame.id, result: { ack: true } }) + '\n');
  } else if (frame.method === 'agent.shutdown') {
    process.stdout.write(JSON.stringify({ kind: 'response', version: 2, id: frame.id, result: {} }) + '\n');
    process.exit(0);
  }
});"#;

    /// 假 Agent：收到 `agent.prompt` 时**先**回 `response`，**再**发 5 条 `agent.run`
    /// 事件。这样 `request` 一读完 ack 就返回，事件只能靠泵主动读取。
    const EVENTS_AFTER_ACK_AGENT: &str = r#"import readline from 'node:readline';
const input = readline.createInterface({ input: process.stdin });
const emit = (event, payload) => process.stdout.write(JSON.stringify({ kind: 'event', version: 2, event, payload }) + '\n');
input.on('line', (line) => {
  const frame = JSON.parse(line);
  if (frame.kind === 'hello') {
    process.stdout.write(JSON.stringify({ kind: 'hello', version: 2, protocol: 'copper-addon.ndjson', supported_versions: [2], runtime: 'test-agent', capabilities: [] }) + '\n');
  } else if (frame.method === 'agent.prompt') {
    process.stdout.write(JSON.stringify({ kind: 'response', version: 2, id: frame.id, result: { ack: true } }) + '\n');
    for (let i = 0; i < 5; i++) emit('agent.run', { runId: 'r2', sequence: i });
  } else if (frame.method === 'agent.shutdown') {
    process.stdout.write(JSON.stringify({ kind: 'response', version: 2, id: frame.id, result: {} }) + '\n');
    process.exit(0);
  }
});"#;

    #[test]
    fn pump_events_drains_the_events_buffered_while_a_request_waited() {
        let temp = TempDir::new();
        let entry = temp.0.join("agent.mjs");
        fs::write(&entry, EVENTS_BEFORE_ACK_AGENT).unwrap();

        let mut session = super::NodeRuntimeSession::launch(
            &node_executable(),
            &entry,
            Arc::new(NoCapabilities),
            "copper-lamp.agent",
        )
        .unwrap();

        let ack = session
            .request(METHOD_AGENT_PROMPT, serde_json::json!({ "prompt": "hi" }))
            .unwrap();
        assert_eq!(ack["ack"], serde_json::json!(true));
        assert_eq!(
            session.pending_events.len(),
            5,
            "响应之前的 5 条事件必须在等待响应期间被缓冲下来"
        );

        let mut seen: Vec<u64> = Vec::new();
        let mut callbacks = 0usize;

        // 预算小于缓冲量：连拉三次，验证预算生效、且跨次调用顺序不乱。
        let first = pump_into(&mut session, 2, &mut seen, &mut callbacks);
        assert_eq!(first, 2);
        assert_eq!(seen, vec![0, 1]);

        let second = pump_into(&mut session, 2, &mut seen, &mut callbacks);
        assert_eq!(second, 2);
        assert_eq!(seen, vec![0, 1, 2, 3]);

        let third = pump_into(&mut session, 2, &mut seen, &mut callbacks);
        assert_eq!(third, 1, "缓冲只剩最后一条");

        assert_eq!(callbacks, 5, "投递条数必须与回调次数一致");
        assert_eq!(seen, vec![0, 1, 2, 3, 4], "缓冲事件必须按到达顺序投递");
        assert!(session.pending_events.is_empty());
        assert_eq!(session.pending_event_bytes, 0, "缓冲字节计数必须同步归零");

        session.shutdown(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn pump_events_reads_events_that_arrive_after_the_response() {
        let temp = TempDir::new();
        let entry = temp.0.join("agent.mjs");
        fs::write(&entry, EVENTS_AFTER_ACK_AGENT).unwrap();

        let mut session = super::NodeRuntimeSession::launch(
            &node_executable(),
            &entry,
            Arc::new(NoCapabilities),
            "copper-lamp.agent",
        )
        .unwrap();

        let ack = session
            .request(METHOD_AGENT_PROMPT, serde_json::json!({ "prompt": "hi" }))
            .unwrap();
        assert_eq!(ack["ack"], serde_json::json!(true));
        assert!(
            session.pending_events.is_empty(),
            "响应先到，事件不可能已被缓冲"
        );

        let mut seen: Vec<u64> = Vec::new();
        let mut callbacks = 0usize;
        let delivered = session
            .pump_events(Duration::from_millis(500), 64, &mut |frame| {
                if let WireFrame::Event { payload, .. } = frame {
                    callbacks += 1;
                    seen.push(payload["sequence"].as_u64().unwrap());
                }
            })
            .unwrap();

        assert_eq!(delivered, 5);
        assert_eq!(callbacks, 5, "投递条数必须与回调次数一致");
        assert_eq!(seen, vec![0, 1, 2, 3, 4], "新读事件必须保持到达顺序");

        session.shutdown(Duration::from_secs(2)).unwrap();
    }
}
