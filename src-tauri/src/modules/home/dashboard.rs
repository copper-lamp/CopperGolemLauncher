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
    /// 世界封面（`data:image/...;base64,...`）；服务器没有封面。
    pub image_data_url: Option<String>,
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
        out.push(DashboardEntry {
            id: format!("world:{version}:{folder}"),
            kind: DashboardKind::World,
            name,
            version: version.to_string(),
            detail: folder.clone(),
            image_data_url: world_icon(&path),
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

fn world_icon(dir: &Path) -> Option<String> {
    for (file, mime) in [
        ("world_icon.jpeg", "image/jpeg"),
        ("world_icon.png", "image/png"),
    ] {
        let Ok(raw) = std::fs::read(dir.join(file)) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        return Some(format!("data:{mime};base64,{b64}"));
    }
    None
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
    fn skips_blank_and_comment_lines() {
        let list = parse_external_servers("\n# comment\n\n");
        assert!(list.is_empty());
    }
}
