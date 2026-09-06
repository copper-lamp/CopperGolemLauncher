//! 内核统一 HTTP 客户端构造。
//!
//! 出站请求默认应用代理策略，提升在需要代理的网络（登录、游戏与内容下载的海外源）下的可达性。
//! 代理优先级：环境变量代理（`HTTPS_PROXY/HTTP_PROXY/ALL_PROXY`，含小写变体） → 直连。
//! 各模块复用本构造，避免各自构建客户端而绕过代理设置。

use std::time::Duration;

use reqwest::{Client, ClientBuilder};

/// 构建代理感知的内核 HTTP 客户端（带默认 UA）。
pub fn build_client(timeout: Duration) -> Client {
    client_builder(timeout)
        .user_agent(concat!("copper-golem/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("failed to build reqwest client")
}

/// 把 Windows 系统代理注入环境变量，供只读环境变量的下游（下载引擎等）一并生效。
/// 仅当未显式配置环境变量代理时注入，避免覆盖用户显式设置。应在启动早期、单线程阶段调用。
pub fn inject_system_proxy_env() {
    if env_proxy().is_some() {
        return;
    }
    #[cfg(windows)]
    if let Some(url) = windows_system_proxy() {
        unsafe {
            std::env::set_var("HTTPS_PROXY", url);
        }
    }
}

/// 应用代理策略后的 `ClientBuilder`，调用方可再叠加 UA / 连接参数。
pub fn client_builder(timeout: Duration) -> ClientBuilder {
    let builder = Client::builder()
        .timeout(timeout)
        .connect_timeout(std::time::Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::limited(10));
    apply_proxy(builder)
}

/// 应用代理策略：优先显式环境变量代理，其次 Windows 系统代理（IE/Clash 写注册表），缺失则直连。
fn apply_proxy(mut builder: ClientBuilder) -> ClientBuilder {
    let proxy = env_proxy().or_else(windows_system_proxy);
    if let Some(url) = proxy {
        if let Ok(p) = reqwest::Proxy::all(url) {
            builder = builder.proxy(p);
        }
    }
    builder
}

/// 读取环境变量中的代理地址（无值返回 `None` → 继续回退）。
fn env_proxy() -> Option<String> {
    for var in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 读取 Windows 系统代理（`Internet Settings` 注册表）。
/// Clash 等「系统代理」开启时会写入 `ProxyEnable=1` 与 `ProxyServer`，reqwest 默认不读取它。
#[cfg(windows)]
fn windows_system_proxy() -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    let sub_key = HSTRING::from(KEY);

    // 系统代理开关（ProxyEnable）。
    let mut enabled = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &sub_key,
            &HSTRING::from("ProxyEnable"),
            RRF_RT_REG_DWORD,
            Some(std::ptr::null_mut()),
            Some((&mut enabled as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS || enabled == 0 {
        return None;
    }

    // 代理服务器值（REG_SZ，如 `127.0.0.1:7890` 或 `http=...;https=...;ftp=...`）。
    let mut size = 0u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &sub_key,
            &HSTRING::from("ProxyServer"),
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
            HKEY_CURRENT_USER,
            &sub_key,
            &HSTRING::from("ProxyServer"),
            RRF_RT_REG_SZ,
            Some(std::ptr::null_mut()),
            Some(buf.as_mut_ptr().cast()),
            Some(&mut actual),
        );
    };
    let text = String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string();
    parse_proxy_server(&text)
}

/// 把 `ProxyServer` 值解析成 `http://host:port` 全协议代理 URL。
#[cfg(windows)]
fn parse_proxy_server(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.contains(';') {
        for part in raw.split(';') {
            if let Some((scheme, addr)) = part.split_once('=') {
                if scheme.trim().eq_ignore_ascii_case("https") && !addr.trim().is_empty() {
                    return Some(format!("http://{}", addr.trim()));
                }
            }
        }
        // 无协议段时的兜底取第一段。
        let first = raw.split(';').next()?.trim();
        let addr = first.split_once('=').map(|(_, v)| v.trim()).unwrap_or(first);
        return if addr.is_empty() { None } else { Some(format!("http://{addr}")) };
    }
    Some(format!("http://{raw}"))
}

/// 非 Windows：无系统代理注册表，直接返回 None。
#[cfg(not(windows))]
fn windows_system_proxy() -> Option<String> {
    None
}