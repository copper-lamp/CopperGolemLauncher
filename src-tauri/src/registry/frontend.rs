//! 附加模块前端产物的运行期装配支持：自定义协议服务 + 前端入口列举。
//!
//! # 为什么需要
//!
//! 内置模块的前端包在编译期被内核静态 `import`（`frontend/src/modules/index.ts`），
//! 附加模块在**运行期**才出现在磁盘上，无法静态 import。故：
//!
//! 1. 内核注册自定义协议 `cglmod`，把 `<modules_dir>/<id>/frontend/**` 暴露为
//!    HTTP 资源（Windows/安卓形如 `http://cglmod.localhost/<id>/register.js`，
//!    其它平台形如 `cglmod://localhost/<id>/register.js`）；
//! 2. 前端 Shell 启动后向 `modules_frontends` 取入口清单，逐个以 `<script>`
//!    注入执行；模块脚本经宿主桥 `window.__COPPER_HOST__` 调用 `registerModule`。
//!
//! # 安全
//!
//! 协议处理器把「模块 id → 目录」的解析限制在 `modules_dir` 内，并复用
//! [`crate::registry::package::safe_relative_path`] 拒绝穿越 / 绝对路径 / 保留名。

use std::borrow::Cow;
use std::path::Path;

use serde::Serialize;
use tauri::Manager;

use crate::registry::modules::is_valid_module_id;
use crate::registry::package::safe_relative_path;
use crate::state::KernelContext;

/// 自定义协议名（前端 URL 的 scheme）。
pub const SCHEME: &str = "cglmod";

/// 当前平台的自定义协议 URL 前缀。
///
/// Windows / 安卓的 WebView 把自定义协议映射到 `http://<scheme>.localhost`；
/// 其它平台保留原 scheme。前端无需区分平台——URL 由内核返回。
#[cfg(any(target_os = "windows", target_os = "android"))]
const URL_PREFIX: &str = "http://cglmod.localhost";
#[cfg(not(any(target_os = "windows", target_os = "android")))]
const URL_PREFIX: &str = "cglmod://localhost";

/// 一个附加模块的前端入口（供前端 Shell 注入）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AddonFrontendView {
    /// 模块完整 id。
    pub id: String,
    /// i18n 命名空间（单段）。
    pub namespace: String,
    /// 入口脚本 URL（对应清单 `frontend.register`）。
    pub entry_url: String,
    /// 需随入口一并加载的样式表 URL（模块 `frontend/` 下的全部 `.css`）。
    pub style_urls: Vec<String>,
}

/// 列举可用的附加模块前端入口（已启用、且磁盘上确实存在入口脚本）。
pub fn list_frontends(kernel: &KernelContext) -> Vec<AddonFrontendView> {
    let modules_dir = kernel.paths().modules_dir().clone();
    let Ok(entries) = std::fs::read_dir(&modules_dir) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(id) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_valid_module_id(id) {
            continue;
        }
        if !kernel.modules().is_enabled(kernel, id) {
            continue;
        }
        // 读清单拿命名空间与入口名。
        let Ok(raw) = std::fs::read(dir.join("module.json")) else {
            continue;
        };
        let Ok(manifest) = crate::registry::manifest::ModuleManifest::parse(&raw) else {
            continue;
        };
        let register = manifest.frontend.register.trim();
        if register.is_empty() {
            continue;
        }
        let frontend_dir = dir.join("frontend");
        if !frontend_dir.join(register).is_file() {
            continue;
        }

        let style_urls = collect_styles(&frontend_dir)
            .into_iter()
            .map(|rel| format!("{URL_PREFIX}/{id}/{rel}"))
            .collect();

        out.push(AddonFrontendView {
            id: id.to_string(),
            namespace: manifest.i18n_namespace.clone(),
            entry_url: format!("{URL_PREFIX}/{id}/{}", register.replace('\\', "/")),
            style_urls,
        });
    }

    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// 递归收集 `<frontend>/` 下的全部 `.css`，返回 **URL 风格**的相对路径
/// （分隔符统一为正斜杠，可直接拼进 `cglmod` URL）。
fn collect_styles(frontend_dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("css") {
                if let Ok(rel) = path.strip_prefix(base) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(frontend_dir, frontend_dir, &mut out);
    out.sort();
    out
}

/// 自定义协议处理器：把 `cglmod://.../<id>/<relative>` 映射到
/// `<modules_dir>/<id>/frontend/<relative>`。
pub fn serve_asset(
    ctx: &tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Cow<'static, [u8]>> {
    let not_found = || {
        tauri::http::Response::builder()
            .status(tauri::http::StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(&b"not found"[..]))
            .expect("构造 404 响应")
    };

    let kernel = ctx.app_handle().state::<KernelContext>();
    let modules_dir = kernel.paths().modules_dir().clone();
    drop(kernel);

    let path = request.uri().path().trim_start_matches('/');
    let mut parts = path.splitn(2, '/');
    let id = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    if !is_valid_module_id(id) || rest.is_empty() {
        return not_found();
    }
    let Some(relative) = safe_relative_path(rest) else {
        return not_found();
    };
    // 只允许读取模块的 frontend 目录。
    let file = modules_dir.join(id).join("frontend").join(&relative);
    match std::fs::read(&file) {
        Ok(bytes) => tauri::http::Response::builder()
            .status(tauri::http::StatusCode::OK)
            .header(tauri::http::header::CONTENT_TYPE, mime_for(&relative))
            .body(Cow::Owned(bytes))
            .unwrap_or_else(|_| not_found()),
        Err(_) => not_found(),
    }
}

/// 按扩展名给出 Content-Type（未知回退 `application/octet-stream`）。
fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("js") | Some("mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("html") => "text/html",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_covers_expected_extensions() {
        assert_eq!(mime_for(Path::new("a.js")), "text/javascript");
        assert_eq!(mime_for(Path::new("a.css")), "text/css");
        assert_eq!(mime_for(Path::new("a.json")), "application/json");
        assert_eq!(mime_for(Path::new("a.bin")), "application/octet-stream");
    }

    #[test]
    fn collect_styles_is_recursive_and_sorted() {
        let dir = std::env::temp_dir().join(format!("cgl-fe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("x.css"), b"").unwrap();
        std::fs::write(dir.join("assets/y.css"), b"").unwrap();
        std::fs::write(dir.join("register.js"), b"").unwrap();

        let styles = collect_styles(&dir);
        assert_eq!(styles, vec!["assets/y.css".to_string(), "x.css".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
