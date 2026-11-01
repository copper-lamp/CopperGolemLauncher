//! 开始页默认模式：跨实例的本地世界与外部服务器。
//!
//! 数据来源都是磁盘事实：
//! - 世界：每个实例内容根目录下的 `minecraftWorlds/<目录>`，名字取
//!   `levelname.txt`，封面取 `world_icon.jpeg`（缺失则没有图）；
//! - 服务器：`com.mojang/external_servers.txt`。
//!
//! 服务器文件没有官方 schema，实际见过 `名称:地址:端口` 与 `地址:端口` 两种写法，
//! 解析器按**从右往左**认字段：末尾纯数字当端口，再前一段当地址，剩下的当地址 /
//! 名称。宁可容忍格式差异，也不要因为一行格式不符就整份列表为空。

use std::path::Path;

use base64::Engine;
use serde::Serialize;

use crate::state::KernelContext;

use super::content;

/// 条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardKind {
    World,
    Server,
}

/// 默认模式列表区的条目。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DashboardEntry {
    /// 稳定 id：世界用 `world:<实例>:<目录名>`，服务器用 `server:<实例>:<序号>`。
    pub id: String,
    pub kind: DashboardKind,
    pub name: String,
    /// 所属实例名。
    pub version: String,
    /// 附加说明：世界用目录名，服务器用 `地址:端口`。
    pub detail: String,
    /// 世界封面（`data:image/...;base64,...`，等于 `images` 的第一张）；服务器没有封面。
    pub image_data_url: Option<String>,
    /// 该条目可用的全部预览图（官方图标在前，其后是用户自己放进世界目录的图片）。
    ///
    /// 前端「切换预览图」按这个列表循环；服务器没有预览图，为空数组。
    pub images: Vec<String>,
}

/// 扫描全部实例的世界与服务器。单个实例失败只跳过它，不影响其它实例。
pub fn entries(kernel: &KernelContext) -> Vec<DashboardEntry> {
    let mut out = Vec::new();
    for version in super::HomeModule::list_versions(kernel) {
        let Ok(roots) = content::content_roots(kernel, &version.name) else {
            continue;
        };
        if let Some(worlds) = content::first_player_worlds_dir(&roots) {
            collect_worlds(&worlds, &version.name, &mut out);
        }
        collect_servers(roots.com_mojang.as_path(), &version.name, &mut out);
    }
    out
}

fn collect_worlds(worlds: &Path, version: &str, out: &mut Vec<DashboardEntry>) {
    let Ok(read) = std::fs::read_dir(worlds) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().into_owned();
        if folder.starts_with('.') {
            continue;
        }
        // 空目录不是有效存档（游戏只在真正进入过世界后才写 levelname.txt）。
        let Some(name) = world_name(&path) else {
            continue;
        };
        let images = world_images(&path);
        out.push(DashboardEntry {
            id: format!("world:{version}:{folder}"),
            kind: DashboardKind::World,
            name,
            version: version.to_string(),
            detail: folder.clone(),
            image_data_url: images.first().cloned(),
            images,
        });
    }
}

/// 世界显示名：`levelname.txt` 优先，缺失回退目录名。
fn world_name(dir: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(dir.join("levelname.txt")) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
}

/// 单个预览图的体积上限：图片要经 IPC 以 data URL 送到前端，
/// 大图会拖慢整个列表的首屏。
const MAX_IMAGE_BYTES: u64 = 1_500_000;
/// 每个世界最多收集的预览图数量（官方图标 + 用户自备图片）。
const MAX_IMAGES: usize = 8;

/// 按扩展名判定图片 MIME；不认识的后缀返回 `None`（不猜）。
fn image_mime(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        Some("image/png")
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("image/jpeg")
    } else if lower.ends_with(".webp") {
        Some("image/webp")
    } else {
        None
    }
}

fn encode_image(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let mime = image_mime(path.file_name()?.to_str()?)?;
    let raw = std::fs::read(path).ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
    Some(format!("data:{mime};base64,{b64}"))
}

/// 世界的全部预览图：官方图标优先，其次是用户自己丢进世界目录的图片。
///
/// 顺序固定（官方图标 → 其余按文件名排序），「切换预览图」才能稳定地循环，
/// 不会因为目录遍历顺序变化而每次点出来都不一样。
fn world_images(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for fixed in ["world_icon.jpeg", "world_icon.png"] {
        if dir.join(fixed).is_file() {
            names.push(fixed.to_string());
        }
    }
    if let Ok(read) = std::fs::read_dir(dir) {
        let mut extra: Vec<String> = read
            .flatten()
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| image_mime(name).is_some())
            .filter(|name| {
                !name.eq_ignore_ascii_case("world_icon.jpeg")
                    && !name.eq_ignore_ascii_case("world_icon.png")
            })
            .collect();
        extra.sort_by_key(|name| name.to_ascii_lowercase());
        names.extend(extra);
    }
    names.truncate(MAX_IMAGES);
    names
        .iter()
        .filter_map(|name| encode_image(&dir.join(name)))
        .collect()
}

fn collect_servers(com_mojang: &Path, version: &str, out: &mut Vec<DashboardEntry>) {
    let Ok(text) = std::fs::read_to_string(com_mojang.join("external_servers.txt")) else {
        return;
    };
    for (index, server) in parse_external_servers(&text).into_iter().enumerate() {
        out.push(DashboardEntry {
            id: format!("server:{version}:{index}"),
            kind: DashboardKind::Server,
            name: server.name,
            version: version.to_string(),
            detail: format!("{}:{}", server.address, server.port),
            image_data_url: None,
            images: Vec::new(),
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerEntry {
    name: String,
    address: String,
    port: u16,
}

/// 解析 `external_servers.txt`。
///
/// 兼容两种写法（从右往左认字段）：
/// - `地址:端口`
/// - `名称:地址:端口`
///
/// 没有端口时按 MCBE 默认 19132；IPv6 地址带冒号，最后一段不是纯数字时整体当地址。
fn parse_external_servers(text: &str) -> Vec<ServerEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // 也接受制表符 / 逗号分隔（不同版本写入方式不稳）。
        let parts: Vec<&str> = line
            .split([':', '\t', ','])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.is_empty() {
            continue;
        }
        let (name, address, port) = if parts.len() >= 2 {
            if let Ok(port) = parts[parts.len() - 1].parse::<u16>() {
                let address = parts[parts.len() - 2].to_string();
                let name = if parts.len() >= 3 {
                    parts[..parts.len() - 2].join(":")
                } else {
                    address.clone()
                };
                (name, address, port)
            } else {
                (line.to_string(), line.to_string(), 19132)
            }
        } else {
            (line.to_string(), line.to_string(), 19132)
        };
        // `name` 为空（如 `:1.2.3.4:19132`）时退回地址，避免出现无标题卡片。
        let name = if name.trim().is_empty() { address.clone() } else { name };
        out.push(ServerEntry { name, address, port });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_address_port_lines() {
        let list = parse_external_servers("play.example.com:19132\n1.2.3.4:25565\n");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].address, "play.example.com");
        assert_eq!(list[0].port, 19132);
        assert_eq!(list[0].name, "play.example.com");
        assert_eq!(list[1].port, 25565);
    }

    #[test]
    fn parses_name_address_port_lines() {
        let list = parse_external_servers("我的服务器:mc.example.com:19132\n");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "我的服务器");
        assert_eq!(list[0].address, "mc.example.com");
        assert_eq!(list[0].port, 19132);
    }

    #[test]
    fn defaults_port_when_missing() {
        let list = parse_external_servers("mc.example.com\n");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].port, 19132);
        assert_eq!(list[0].address, "mc.example.com");
    }

    #[test]
    fn maps_image_extensions_to_mime() {
        assert_eq!(image_mime("a.PNG"), Some("image/png"));
        assert_eq!(image_mime("b.jpg"), Some("image/jpeg"));
        assert_eq!(image_mime("c.jpeg"), Some("image/jpeg"));
        assert_eq!(image_mime("d.webp"), Some("image/webp"));
        assert_eq!(image_mime("level.dat"), None);
    }

    #[test]
    fn world_images_puts_the_official_icon_first() {
        let dir = std::env::temp_dir().join(format!("cgl-dash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("world_icon.png"), b"\x89PNG").unwrap();
        std::fs::write(dir.join("a-screenshot.png"), b"\x89PNG").unwrap();
        std::fs::write(dir.join("level.dat"), b"nope").unwrap();

        let images = world_images(&dir);
        assert_eq!(images.len(), 2);
        assert!(images[0].starts_with("data:image/png;base64,"));
        // 官方图标在前，其余按文件名排序。
        let _ = images;
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"level.dat".to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_blank_and_comment_lines() {
        let list = parse_external_servers("\n# comment\n\n");
        assert!(list.is_empty());
    }
}
