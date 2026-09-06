//! 内容下载模块 · 共享 HTTP 客户端。
//!
//! 单一 `reqwest::Client` 复用连接池（默认站内常驻，配合来源 API 频控），
//! 携带统一 UA；来源各自经它发起请求，避免每调用新建连接。

use std::sync::OnceLock;

use reqwest::Client;

/// 模块统一 User-Agent，标明产品与用途，便于来源站识别。
const USER_AGENT: &str = concat!(
    "copper-golem/",
    env!("CARGO_PKG_VERSION"),
    " (content-download; +curseforge/lipr)"
);

/// 全局复用的 HTTP 客户端（惰性初始化一次）。
pub fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        crate::services::http_client::client_builder(std::time::Duration::from_secs(30))
            .user_agent(USER_AGENT)
            .build()
            .expect("构建内容下载 HTTP 客户端失败")
    })
}