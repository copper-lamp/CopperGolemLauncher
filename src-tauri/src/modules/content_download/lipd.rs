//! 内容下载模块 · lipd 运行时与 JSON-RPC 基础设施。
//!
//! 现代 lip（npm 包 `@futrime/lip`）只分发守护进程 `lipd`，**不再提供 `lip install`
//! 子命令**：安装 / 更新 / 卸载 / 列举都必须以 `<exe> run` 启动守护进程，经 stdin/stdout
//! 用 `Content-Length` 帧承载 JSON-RPC 2.0 消息交互（与 LSP 同样的帧格式）。
//!
//! 本文件移植 LeviLauncher `internal/lip`（`install.go` / `daemon_rpc.go`）中与运行环境
//! 和 RPC 协议相关的部分：
//!
//! - 可执行发现（PATH → LeviLauncher 兼容目录）
//! - .NET 运行时根定位并注入 `DOTNET_ROOT`（lipd 依赖 .NET 10 运行时）
//! - 中国用户镜像配置（`<AppData>/lip/liprc.json` 的 github / go module 代理）
//! - 帧式 RPC 调用：回调应答、stderr 并发回收、5 分钟超时、2 秒宽限后强杀
//! - `List` 结果解码为「包引用 → 安装状态」索引（兼容多种序列化形态）

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

/// 单次 daemon 调用总超时（与 LeviLauncher `defaultDaemonTimeout` 一致）。
const DAEMON_TIMEOUT: Duration = Duration::from_secs(300);
/// 请求结束后等待进程退出的宽限期，超时强杀（与 LeviLauncher 一致）。
const KILL_GRACE: Duration = Duration::from_secs(2);
/// lipd 依赖的 .NET 运行时主版本。
const DOTNET_MAJOR: u32 = 10;
/// .NET 共享运行时目录名。
const DOTNET_RUNTIME_NAME: &str = "Microsoft.NETCore.App";
/// liprc.json 的格式版本 / UUID 与中国镜像（与 LeviLauncher 一致）。
const CONFIG_FORMAT_VERSION: i64 = 3;
const CONFIG_FORMAT_UUID: &str = "289f771f-2c9a-4d73-9f3f-8492495a924d";
const CONFIG_GITHUB_PROXY: &str = "https://github.bibk.top";
const CONFIG_GO_MODULE_PROXY: &str = "https://goproxy.cn";

/// 隐藏子进程控制台窗口。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ---------------------------------------------------------------- 可执行发现

/// 定位 lipd 可执行文件。
///
/// 查找顺序（与 LeviLauncher `LipExePath` 语义一致）：
/// 1. PATH 中的 `lipd` / `lip`（Windows 按 `PATHEXT` 补扩展名）；
/// 2. LeviLauncher 安装目录下的 `lipd.exe`（`<AppData>/levilauncher.exe/bin/lip/`，
///    及旧版 `<AppData>/levilauncher.exe/bin/`），便于复用用户已装好的 lip。
pub fn find_lip_executable() -> Option<PathBuf> {
    for name in ["lipd", "lip"] {
        if let Some(path) = super::find_executable(name) {
            return Some(path);
        }
    }
    levi_launcher_candidates().into_iter().find(|p| p.is_file())
}

/// lipd 可执行文件名（Windows 为 `lipd.exe`）。
fn lip_binary_name() -> &'static str {
    if cfg!(windows) {
        "lipd.exe"
    } else {
        "lipd"
    }
}

/// LeviLauncher 安装目录下的 lipd 候选路径。
fn levi_launcher_candidates() -> Vec<PathBuf> {
    let Some(base) = app_data_dir() else {
        return Vec::new();
    };
    let bin = base.join("levilauncher.exe").join("bin");
    vec![bin.join("lip").join(lip_binary_name()), bin.join(lip_binary_name())]
}

/// 应用数据目录（与 LeviLauncher `apppath.AppData` 一致）：
/// Windows 取 `%APPDATA%`、回退 `%USERPROFILE%/AppData/Roaming`、再回退临时目录；
/// Unix 取 `$XDG_CACHE_HOME`、回退 `~/.cache`。
fn app_data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(v) = env_non_empty("APPDATA") {
            return Some(PathBuf::from(v));
        }
        if let Some(home) = env_non_empty("USERPROFILE") {
            return Some(PathBuf::from(home).join("AppData").join("Roaming"));
        }
        Some(std::env::temp_dir())
    }
    #[cfg(not(windows))]
    {
        if let Some(v) = env_non_empty("XDG_CACHE_HOME") {
            return Some(PathBuf::from(v));
        }
        env_non_empty("HOME").map(|h| PathBuf::from(h).join(".cache"))
    }
}

/// 读取非空环境变量（两端去空白）。
fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------- .NET 运行时

/// lipd 的 .NET 运行时根目录；未安装返回 `None`。
///
/// 优先 `DOTNET_ROOT`，其次默认安装根（Windows `%LOCALAPPDATA%/Microsoft/dotnet`，
/// Unix `~/.dotnet`；`DOTNET_INSTALL_DIR` 优先于默认根）。
pub fn find_dotnet_root() -> Option<PathBuf> {
    if let Some(root) = env_non_empty("DOTNET_ROOT") {
        let path = PathBuf::from(root);
        if has_dotnet_major_in_root(&path) {
            return Some(path);
        }
    }
    let default_root = dotnet_default_root();
    has_dotnet_major_in_root(&default_root).then_some(default_root)
}

fn dotnet_default_root() -> PathBuf {
    if let Some(dir) = env_non_empty("DOTNET_INSTALL_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    {
        let local = env_non_empty("LocalAppData")
            .or_else(|| env_non_empty("LOCALAPPDATA"))
            .or_else(|| {
                env_non_empty("USERPROFILE")
                    .map(|h| PathBuf::from(h).join("AppData").join("Local"))
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned());
        PathBuf::from(local).join("Microsoft").join("dotnet")
    }
    #[cfg(not(windows))]
    {
        env_non_empty("HOME")
            .map(|h| PathBuf::from(h).join(".dotnet"))
            .unwrap_or_else(|| std::env::temp_dir().join(".dotnet"))
    }
}

/// 判断某安装根下是否存在目标主版本的共享运行时目录（`shared/<runtime>/10.x`）。
fn has_dotnet_major_in_root(root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root.join("shared").join(DOTNET_RUNTIME_NAME)) else {
        return false;
    };
    let prefix = format!("{DOTNET_MAJOR}.");
    entries.flatten().any(|entry| {
        entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
            && entry.file_name().to_string_lossy().trim().starts_with(&prefix)
    })
}

// ---------------------------------------------------------------- 中国镜像配置

/// 中国用户：写 `liprc.json` 的 GitHub / Go module 代理，提升国内安装可达性。
///
/// 采用「合并写入」：只覆盖 4 个受管键，保留用户其它配置。非中国用户直接跳过。
fn ensure_runtime_config() -> Result<(), String> {
    if !is_china_user() {
        return Ok(());
    }
    let Some(path) = app_data_dir().map(|d| d.join("lip").join("liprc.json")) else {
        return Ok(());
    };

    let mut config: Map<String, Value> = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => map,
            _ => {
                log::warn!("[content-download] liprc.json 内容非法，已重置：{}", path.display());
                Map::new()
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(format!("读取 lip 配置失败：{e}")),
    };

    let desired = [
        ("format_version", Value::from(CONFIG_FORMAT_VERSION)),
        ("format_uuid", Value::from(CONFIG_FORMAT_UUID)),
        ("github_proxy", Value::from(CONFIG_GITHUB_PROXY)),
        ("go_module_proxy", Value::from(CONFIG_GO_MODULE_PROXY)),
    ];
    let mut changed = false;
    for (key, value) in desired {
        if config.get(key).is_some_and(|current| same_json(current, &value)) {
            continue;
        }
        config.insert(key.to_string(), value);
        changed = true;
    }
    if !changed {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 lip 配置目录失败：{e}"))?;
    }
    let mut text =
        serde_json::to_string_pretty(&Value::Object(config)).map_err(|e| format!("序列化 lip 配置失败：{e}"))?;
    text.push('\n');
    std::fs::write(&path, text).map_err(|e| format!("写入 lip 配置失败：{e}"))?;
    Ok(())
}

/// JSON 值等价比较（按规范化文本比较，避免数字形态差异造成误判）。
fn same_json(a: &Value, b: &Value) -> bool {
    a == b || serde_json::to_string(a).ok() == serde_json::to_string(b).ok()
}

/// 是否中国用户：时区为中国标准时间，或系统 / 环境语言为简体中文。
fn is_china_user() -> bool {
    #[cfg(windows)]
    {
        is_windows_time_zone_china() || is_windows_locale_zh_cn()
    }
    #[cfg(not(windows))]
    {
        is_unix_time_zone_china() || env_locale_is_zh_cn()
    }
}

/// 语言标签是否为简体中文（覆盖 `zh-CN`、`zh-Hans`、LCID `0804` 等写法）。
fn is_zh_cn_locale(raw: &str) -> bool {
    let lowered = raw.trim().to_lowercase().replace('_', "-");
    let s = lowered.trim_start_matches("0x");
    if s.is_empty() {
        return false;
    }
    if s.starts_with("zh-cn") || s == "zh" || s == "zh-hans" {
        return true;
    }
    if s.contains("chinese (simplified") {
        return true;
    }
    matches!(s, "804" | "0804" | "00000804")
}

fn env_locale_is_zh_cn() -> bool {
    ["LANG", "LANGUAGE", "LC_ALL", "LC_MESSAGES"]
        .iter()
        .any(|key| std::env::var(key).is_ok_and(|v| is_zh_cn_locale(&v)))
}

#[cfg(not(windows))]
fn is_unix_time_zone_china() -> bool {
    if let Ok(content) = std::fs::read_to_string("/etc/timezone") {
        let s = content.trim().to_lowercase();
        if s == "asia/shanghai" || s == "asia/urumqi" {
            return true;
        }
    } else if let Ok(link) = std::fs::read_link("/etc/localtime") {
        let low = link.to_string_lossy().to_lowercase();
        if low.contains("asia/shanghai") || low.contains("asia/urumqi") {
            return true;
        }
    }
    false
}

/// 读取注册表字符串值（`REG_SZ`），失败返回 `None`。
#[cfg(windows)]
fn reg_string(
    root: windows::Win32::System::Registry::HKEY,
    sub_key: &str,
    value: &str,
) -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};

    let sub_key = HSTRING::from(sub_key);
    let value = HSTRING::from(value);

    let mut size = 0u32;
    let rc = unsafe {
        RegGetValueW(
            root,
            &sub_key,
            &value,
            RRF_RT_REG_SZ,
            Some(std::ptr::null_mut()),
            Some(std::ptr::null_mut()),
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS || size == 0 {
        return None;
    }

    // size 含结尾 NUL，按 char 宽凑足 2 的倍数并留安全余量。
    let mut buf = vec![0u16; (size as usize / 2) + 2];
    let mut actual = (buf.len() * 2) as u32;
    unsafe {
        let _ = RegGetValueW(
            root,
            &sub_key,
            &value,
            RRF_RT_REG_SZ,
            Some(std::ptr::null_mut()),
            Some(buf.as_mut_ptr().cast()),
            Some(&mut actual),
        );
    }
    let text = String::from_utf16_lossy(&buf)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(windows)]
fn is_windows_time_zone_china() -> bool {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;

    const KEY: &str = r"SYSTEM\CurrentControlSet\Control\TimeZoneInformation";
    ["TimeZoneKeyName", "StandardName"].iter().any(|field| {
        reg_string(HKEY_LOCAL_MACHINE, KEY, field)
            .is_some_and(|v| v.eq_ignore_ascii_case("China Standard Time"))
    })
}

#[cfg(windows)]
fn is_windows_locale_zh_cn() -> bool {
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;

    const KEY: &str = r"Control Panel\International";
    ["LocaleName", "Locale", "sLanguage"]
        .iter()
        .any(|field| reg_string(HKEY_CURRENT_USER, KEY, field).is_some_and(|v| is_zh_cn_locale(&v)))
        || env_locale_is_zh_cn()
}

// ---------------------------------------------------------------- 包安装状态

/// 单个包在目标版本目录中的安装状态。
#[derive(Debug, Clone, Default)]
pub struct PackageState {
    pub installed: bool,
    pub explicit_installed: bool,
    pub installed_version: String,
}

/// 经 daemon `List` 查询目标目录的安装状态索引（键为小写包引用 `path#variant`）。
///
/// 查询失败返回 `Err(消息)`，由调用方决定是否忽略（与 LeviLauncher 一致，失败不阻断安装）。
pub async fn list_package_states(
    exe: &Path,
    work_dir: &Path,
) -> Result<HashMap<String, PackageState>, String> {
    let output = call(exe, work_dir, "List", Value::Array(Vec::new()))
        .await
        .map_err(|f| f.message)?;
    let groups = decode_spec_groups(&output.result)
        .ok_or_else(|| format!("无法解析 lipd 返回的包列表：{}", preview(&output.result)))?;
    Ok(build_state_index(&groups))
}

/// 经 daemon 安装包（`Install [packages, false, false, false]`）。
pub async fn install_packages(
    exe: &Path,
    work_dir: &Path,
    packages: &[String],
) -> Result<Vec<String>, DaemonFailure> {
    let params = serde_json::json!([packages, false, false, false]);
    call(exe, work_dir, "Install", params).await.map(|o| o.logs)
}

/// 经 daemon 更新包（`Update [packages, false, false]`）。
pub async fn update_packages(
    exe: &Path,
    work_dir: &Path,
    packages: &[String],
) -> Result<Vec<String>, DaemonFailure> {
    let params = serde_json::json!([packages, false, false]);
    call(exe, work_dir, "Update", params).await.map(|o| o.logs)
}

/// daemon 调用成功结果。
#[derive(Debug, Clone)]
pub struct DaemonOutput {
    /// RPC 返回的 `result`（无结果为 `null`）。
    pub result: Value,
    /// 收集到的 daemon 日志回调文本。
    pub logs: Vec<String>,
}

/// daemon 调用失败。
#[derive(Debug, Clone)]
pub struct DaemonFailure {
    /// 失败原因（含 RPC 错误 / 进程退出 / stderr 摘要）。
    pub message: String,
    /// 失败前收集到的 daemon 日志回调文本。
    pub logs: Vec<String>,
}

// ---------------------------------------------------------------- 帧式 RPC

/// 以 `<exe> run` 启动 lipd，发一条 JSON-RPC 请求并同步等待其结果。
///
/// 协议要点（与 LeviLauncher `callDaemonWithResultInternal` 一致）：
/// - 报文用 `Content-Length` 帧封装；
/// - daemon 主动发来的 `PrintInfo` / `PrintSuccess` / `PrintWarning` / `PrintError` /
///   `ReportProgress` 回调需逐条回 `{"jsonrpc":"2.0","id":<id>,"result":null}`，
///   否则 daemon 会阻塞等待应答；
/// - stderr 需并发读取，避免管道写满导致死锁；
/// - 超时 5 分钟；结束后关闭 stdin，等待 2 秒仍未退出则强杀。
pub async fn call(
    exe: &Path,
    work_dir: &Path,
    method: &str,
    params: Value,
) -> Result<DaemonOutput, DaemonFailure> {
    if let Err(message) = ensure_runtime_config() {
        return Err(DaemonFailure {
            message,
            logs: Vec::new(),
        });
    }

    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("run")
        .current_dir(work_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    if let Some(root) = find_dotnet_root() {
        cmd.env("DOTNET_ROOT", root);
    }

    let mut child = cmd.spawn().map_err(|e| DaemonFailure {
        message: format!("启动 lipd 失败：{e}"),
        logs: Vec::new(),
    })?;
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(DaemonFailure {
            message: "无法接管 lipd 的标准输入输出管道".into(),
            logs: Vec::new(),
        });
    };

    // stderr 并发读干：lipd 失败时会写入较多诊断信息，必须及时排空管道。
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).trim().to_string()
    });

    let exchange = match tokio::time::timeout(
        DAEMON_TIMEOUT,
        rpc_exchange(&mut stdin, stdout, method, &params),
    )
    .await
    {
        Ok(exchange) => exchange,
        Err(_) => Exchange {
            result: None,
            logs: Vec::new(),
            error: Some(format!("lipd {method} 超时（{} 秒）", DAEMON_TIMEOUT.as_secs())),
        },
    };

    // 关闭 stdin 通知 daemon 结束，随后限时等待退出。
    let _ = stdin.shutdown().await;
    drop(stdin);
    let exited = matches!(
        tokio::time::timeout(KILL_GRACE, child.wait()).await,
        Ok(Ok(_))
    );
    if !exited {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    let stderr_text = stderr_task.await.unwrap_or_default();

    match exchange.error {
        None => Ok(DaemonOutput {
            result: exchange.result.unwrap_or(Value::Null),
            logs: exchange.logs,
        }),
        Some(message) => Err(DaemonFailure {
            message: if stderr_text.is_empty() {
                message
            } else {
                format!("{message}；lipd 输出：{stderr_text}")
            },
            logs: exchange.logs,
        }),
    }
}

/// 单次 RPC 交互的结果（成功与失败共用，失败时保留已收集日志）。
#[derive(Default)]
struct Exchange {
    result: Option<Value>,
    logs: Vec<String>,
    error: Option<String>,
}

async fn rpc_exchange(
    stdin: &mut ChildStdin,
    stdout: ChildStdout,
    method: &str,
    params: &Value,
) -> Exchange {
    let mut exchange = Exchange::default();
    let request_id: i64 = 1;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params,
    });
    let payload = match serde_json::to_vec(&request) {
        Ok(payload) => payload,
        Err(e) => {
            exchange.error = Some(format!("序列化 lipd 请求失败：{e}"));
            return exchange;
        }
    };
    if let Err(e) = write_frame(stdin, &payload).await {
        exchange.error = Some(format!("写入 lipd 请求失败：{e}"));
        return exchange;
    }

    let mut reader = BufReader::new(stdout);
    loop {
        let frame = match read_frame(&mut reader).await {
            Ok(frame) => frame,
            Err(e) => {
                exchange.error = Some(format!("读取 lipd 消息失败：{e}"));
                return exchange;
            }
        };
        let message: Value = match serde_json::from_slice(&frame) {
            Ok(message) => message,
            Err(e) => {
                exchange.error = Some(format!("解析 lipd 消息失败：{e}"));
                return exchange;
            }
        };

        // 带 method 的消息是 daemon 回调，需回执后继续等待。
        let callback_method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !callback_method.is_empty() {
            if let Some(text) = describe_callback(&callback_method, message.get("params")) {
                exchange.logs.push(text);
            }
            if let Some(id) = message.get("id").filter(|v| !v.is_null()) {
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id.clone(),
                    "result": Value::Null,
                });
                match serde_json::to_vec(&response) {
                    Ok(bytes) => {
                        if let Err(e) = write_frame(stdin, &bytes).await {
                            exchange.error = Some(format!("应答 lipd 回调失败：{e}"));
                            return exchange;
                        }
                    }
                    Err(e) => {
                        exchange.error = Some(format!("序列化 lipd 回执失败：{e}"));
                        return exchange;
                    }
                }
            }
            continue;
        }

        if !id_matches(message.get("id"), request_id) {
            continue;
        }
        match message.get("error").filter(|v| !v.is_null()) {
            Some(err) => {
                let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                let text = err.get("message").and_then(Value::as_str).unwrap_or("");
                exchange.error = Some(format!("rpc {method} 失败：code={code} message={text}"));
            }
            None => exchange.result = Some(message.get("result").cloned().unwrap_or(Value::Null)),
        }
        return exchange;
    }
}

/// 写入 `Content-Length` 帧。
async fn write_frame(writer: &mut ChildStdin, payload: &[u8]) -> io::Result<()> {
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", payload.len()).as_bytes())
        .await?;
    writer.write_all(payload).await?;
    writer.flush().await
}

/// 读取一个 `Content-Length` 帧的正文。
async fn read_frame(reader: &mut BufReader<ChildStdout>) -> io::Result<Vec<u8>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "lipd 输出提前结束",
            ));
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            break;
        }
        if let Some((key, value)) = text.split_once(':') {
            if key.trim().eq_ignore_ascii_case("content-length") {
                content_length = Some(value.trim().parse::<usize>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("无效的 Content-Length：{}", value.trim()),
                    )
                })?);
            }
        }
    }
    let length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "lipd 消息缺少 Content-Length 头")
    })?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    Ok(body)
}

/// 请求 id 匹配（daemon 可能回数字、浮点或字符串形态）。
fn id_matches(id: Option<&Value>, want: i64) -> bool {
    match id {
        Some(Value::Number(n)) => {
            n.as_i64() == Some(want) || n.as_f64().is_some_and(|f| f as i64 == want)
        }
        Some(Value::String(s)) => s.trim().parse::<i64>().is_ok_and(|v| v == want),
        _ => false,
    }
}

/// 把 daemon 回调转成一行可读日志；无可读信息返回 `None`。
fn describe_callback(method: &str, params: Option<&Value>) -> Option<String> {
    if let Some(list) = params.and_then(Value::as_array) {
        match method {
            "PrintInfo" | "PrintSuccess" | "PrintWarning" | "PrintError" => {
                if let Some(first) = list.first() {
                    // PrintSuccess 在 lipd 中多为步骤级状态而非最终完成，统一按信息展示。
                    return Some(value_text(first));
                }
            }
            "ReportProgress" if list.len() >= 3 => {
                let percentage = list[2]
                    .as_f64()
                    .or_else(|| list[2].as_str().and_then(|s| s.trim().parse().ok()))
                    .unwrap_or(0.0);
                return Some(format!("{}（{percentage}%）", value_text(&list[1])));
            }
            _ => {}
        }
    }
    let raw = params.map(Value::to_string).unwrap_or_default();
    let raw = raw.trim();
    if raw.is_empty() || raw == "null" {
        None
    } else {
        Some(format!("{method} {raw}"))
    }
}

/// 回调参数的文本化（字符串原样，其余按 JSON 文本）。
fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// JSON 预览（限长，便于日志与错误信息）。
fn preview(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= 256 {
        return text;
    }
    format!("{}...", text.chars().take(256).collect::<String>())
}

// ---------------------------------------------------------------- List 结果解码

/// lipd 返回的一个包规格。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PackageSpec {
    path: String,
    variant: String,
    version: String,
}

/// 解码 `List` 结果。兼容四种形态（与 LeviLauncher `decodePackageSpecGroups` 一致）：
/// 1. 二维数组 `[[显式...], [依赖...]]`；
/// 2. `{"Item1": [...], "Item2": [...]}`（.NET `ValueTuple` 序列化）；
/// 3. 具名键（`Explicit` / `Dependencies` / `TopLevel` / `Transitive` 等别名）；
/// 4. 一维平铺数组（视为全部为显式安装）。
fn decode_spec_groups(raw: &Value) -> Option<Vec<Vec<PackageSpec>>> {
    match raw {
        Value::Array(items) if items.iter().all(Value::is_array) => {
            Some(items.iter().map(|group| parse_specs(group).unwrap_or_default()).collect())
        }
        Value::Array(_) => parse_specs(raw).map(|specs| vec![specs]),
        Value::Object(map) => object_groups(map),
        _ => None,
    }
}

/// 对象形态的 `List` 结果解码。
fn object_groups(map: &Map<String, Value>) -> Option<Vec<Vec<PackageSpec>>> {
    let first = get_ci(map, "Item1");
    let second = get_ci(map, "Item2");
    if first.is_some() || second.is_some() {
        let explicit = first.and_then(parse_specs);
        let indirect = second.and_then(parse_specs);
        if explicit.is_some() || indirect.is_some() {
            return Some(vec![explicit.unwrap_or_default(), indirect.unwrap_or_default()]);
        }
    }

    let explicit = pick_first(
        map,
        &["Item1", "Explicit", "explicit", "TopLevel", "topLevel"],
    );
    let indirect = pick_first(
        map,
        &["Item2", "Dependencies", "dependencies", "Transitive", "transitive"],
    );
    if explicit.is_some() || indirect.is_some() {
        return Some(vec![explicit.unwrap_or_default(), indirect.unwrap_or_default()]);
    }
    None
}

/// 依次尝试若干键名，返回第一个能解析为包规格列表的值。
fn pick_first(map: &Map<String, Value>, keys: &[&str]) -> Option<Vec<PackageSpec>> {
    keys.iter()
        .filter_map(|key| get_ci(map, key))
        .find_map(parse_specs)
}

/// 解析包规格列表：元素可为 `"path#variant@version"` 字符串或 `{Id:{Path,Variant},Version}` 对象。
fn parse_specs(value: &Value) -> Option<Vec<PackageSpec>> {
    let Value::Array(items) = value else {
        return None;
    };
    let mut specs = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::String(text) => {
                if let Some(spec) = parse_spec_from_string(text) {
                    specs.push(spec);
                }
            }
            Value::Object(map) => {
                let spec = spec_from_object(map);
                if !spec.path.is_empty() {
                    specs.push(spec);
                }
            }
            _ => {}
        }
    }
    Some(specs)
}

/// 解析 `"path#variant@version"` 字符串（`#variant` 与 `@version` 均可缺省）。
fn parse_spec_from_string(raw: &str) -> Option<PackageSpec> {
    let mut rest = raw.trim().to_string();
    if rest.is_empty() {
        return None;
    }

    let mut version = String::new();
    if let Some(at) = rest.rfind('@') {
        if at > 0 {
            version = rest[at + 1..].trim().to_string();
            rest = rest[..at].trim().to_string();
        }
    }
    if rest.is_empty() {
        return None;
    }

    let mut path = rest.clone();
    let mut variant = String::new();
    if let Some(hash) = rest.rfind('#') {
        if hash > 0 {
            path = rest[..hash].trim().to_string();
            variant = rest[hash + 1..].trim().to_string();
        }
    }
    if path.is_empty() {
        return None;
    }
    Some(PackageSpec {
        path,
        variant,
        version,
    })
}

/// 从 `{Id:{Path,Variant},Version}` 对象解析包规格（键名大小写不敏感）。
fn spec_from_object(map: &Map<String, Value>) -> PackageSpec {
    let mut spec = PackageSpec::default();
    if let Some(id) = get_ci(map, "Id").and_then(Value::as_object) {
        spec.path = get_ci(id, "Path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        spec.variant = get_ci(id, "Variant")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
    }
    spec.version = get_ci(map, "Version")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    spec
}

/// 键名大小写不敏感查找。
fn get_ci<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    if let Some(value) = map.get(key) {
        return Some(value);
    }
    map.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, value)| value)
}

/// 建立「小写包引用 → 安装状态」索引：第 1 组为显式安装，其余为依赖安装。
fn build_state_index(groups: &[Vec<PackageSpec>]) -> HashMap<String, PackageState> {
    let mut index: HashMap<String, PackageState> = HashMap::new();
    for (position, group) in groups.iter().enumerate() {
        let explicit = position == 0;
        for spec in group {
            let key = spec_id(spec).to_lowercase();
            if key.is_empty() {
                continue;
            }
            let state = index.entry(key).or_default();
            state.installed = true;
            if explicit {
                state.explicit_installed = true;
                if !spec.version.is_empty() {
                    state.installed_version = spec.version.clone();
                }
            } else if state.installed_version.is_empty() {
                state.installed_version = spec.version.clone();
            }
        }
    }
    index
}

/// 包引用 id（`path` 或 `path#variant`）。
fn spec_id(spec: &PackageSpec) -> String {
    if spec.path.is_empty() {
        return String::new();
    }
    if spec.variant.is_empty() {
        spec.path.clone()
    } else {
        format!("{}#{}", spec.path, spec.variant)
    }
}

// ---------------------------------------------------------------- 单测

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_grouped_arrays() {
        let raw = serde_json::json!([
            [{ "Id": { "Path": "github.com/a/b", "Variant": "client" }, "Version": "1.0.0" }],
            [{ "Id": { "Path": "github.com/c/d", "Variant": "" }, "Version": "2.0.0" }]
        ]);
        let groups = decode_spec_groups(&raw).expect("应解析为两组");
        assert_eq!(groups.len(), 2);
        let index = build_state_index(&groups);
        let explicit = index.get("github.com/a/b#client").expect("显式安装项");
        assert!(explicit.installed && explicit.explicit_installed);
        assert_eq!(explicit.installed_version, "1.0.0");
        let indirect = index.get("github.com/c/d").expect("依赖项");
        assert!(indirect.installed && !indirect.explicit_installed);
    }

    #[test]
    fn decode_tuple_object() {
        let raw = serde_json::json!({
            "Item1": ["github.com/a/b#client@1.2.3"],
            "Item2": ["github.com/c/d@0.1.0"]
        });
        let groups = decode_spec_groups(&raw).expect("应解析 ValueTuple 形态");
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[0][0],
            PackageSpec {
                path: "github.com/a/b".into(),
                variant: "client".into(),
                version: "1.2.3".into()
            }
        );
        assert_eq!(groups[1][0].path, "github.com/c/d");
    }

    #[test]
    fn decode_named_keys_and_flat_array() {
        let named = serde_json::json!({
            "Explicit": [{ "id": { "path": "github.com/a/b", "variant": "client" }, "version": "9.9.9" }],
            "Dependencies": []
        });
        let groups = decode_spec_groups(&named).expect("应解析具名键形态");
        assert_eq!(groups[0][0].version, "9.9.9");

        let flat = serde_json::json!(["github.com/x/y#client@3.0.0"]);
        let groups = decode_spec_groups(&flat).expect("应解析平铺数组");
        assert_eq!(groups.len(), 1);
        let index = build_state_index(&groups);
        assert!(index["github.com/x/y#client"].explicit_installed);
    }

    #[test]
    fn decode_unsupported_shape_returns_none() {
        assert!(decode_spec_groups(&serde_json::json!("nope")).is_none());
        assert!(decode_spec_groups(&serde_json::json!({ "unknown": 1 })).is_none());
    }

    #[test]
    fn zh_cn_locale_detection() {
        assert!(is_zh_cn_locale("zh-CN"));
        assert!(is_zh_cn_locale("zh_CN.UTF-8"));
        assert!(is_zh_cn_locale("zh"));
        assert!(is_zh_cn_locale("00000804"));
        assert!(!is_zh_cn_locale("en-US"));
        assert!(!is_zh_cn_locale(""));
    }
}