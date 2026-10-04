//! 预加载清单生成：启动前扫描实例目录，把「要预加载哪些原生 DLL」写成
//! `copper-preload.json` 交给注入侧。
//!
//! # 为什么不让注入侧自己扫 `mods/`
//!
//! 加载器（LeviLamina 等）经 lipd 安装后的落地产物形态，**在本仓与
//! LeviLauncher 仓内都查不到**（lipd 是外部 .NET 进程，协议由上游决定）。
//! 让注入侧去猜目录结构，等于把这份不确定性写死进最底层的 native 代码。
//! 改成「装完 → 启动器探测 → 写清单 → 注入侧只按清单加载」后，不确定性收敛
//! 在可测试、可观测的 Rust 侧：探测不到就如实告诉用户，而不是静默不生效。
//!
//! 清单格式由注入侧定义（`copper_core_hook::manifest`），两侧共用同一份类型，
//! 避免 schema 漂移。

use std::path::{Path, PathBuf};

use copper_core_hook::contract as contract;
use copper_core_hook::manifest::{PreloadEntry, PreloadManifest};

use crate::error::KernelError;
use crate::state::KernelContext;

/// 生成原因：自动探测到条目。
const REASON_AUTO: &str = "auto-detected";
/// 生成原因：外部预加载器接管。
const REASON_EXTERNAL: &str = "external-preloader";
/// 生成原因：没有可预加载的原生模块。
const REASON_NO_LOADER: &str = "no-loader";
/// 加载器名（大小写不敏感匹配）。
const LEVI_LAMINA: &str = "levilamina";

/// 探测结果（供 UI 展示与排障）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PreloadSummary {
    /// 清单里将加载的条目数。
    pub entry_count: usize,
    /// 命中的加载器名（`LeviLamina`），未命中为 `None`。
    pub loader_name: Option<String>,
    /// 被外部预加载器接管时为 `true`。
    pub external_preloader: bool,
}

/// 一次探测的完整结果（清单条目 + 摘要）。
struct Probe {
    summary: PreloadSummary,
    reason: &'static str,
    entries: Vec<PreloadEntry>,
}

/// 探测实例目录里可预加载的原生 DLL。
pub fn detect(kernel: &KernelContext, name: &str) -> Result<PreloadSummary, KernelError> {
    let dir = crate::modules::home::meta::resolve_version_dir(&kernel.versions_root(), name)?;
    Ok(probe(kernel, name, &dir)?.summary)
}

/// 写入清单（原子），返回摘要。
pub fn write_manifest(kernel: &KernelContext, name: &str) -> Result<PreloadSummary, KernelError> {
    let dir = crate::modules::home::meta::resolve_version_dir(&kernel.versions_root(), name)?;
    write_manifest_by_dir(kernel, name, &dir)
}

/// 写入清单（目录已知时用；启动链路已经在 prepare 阶段解析过目录）。
pub fn write_manifest_by_dir(
    kernel: &KernelContext,
    name: &str,
    dir: &Path,
) -> Result<PreloadSummary, KernelError> {
    let probed = probe(kernel, name, dir)?;
    let manifest = PreloadManifest::new(probed.reason, probed.entries);
    let target = super::inject::manifest_path(dir);
    let tmp = target.with_extension("json.tmp");
    std::fs::write(&tmp, manifest.to_json())?;
    std::fs::rename(&tmp, &target)?;
    Ok(probed.summary)
}

/// 探测实现（[`detect`] 与 [`write_manifest_by_dir`] 共用，避免两套规则）。
fn probe(
    kernel: &KernelContext,
    name: &str,
    dir: &Path,
) -> Result<Probe, KernelError> {
    // 外部预加载器接管：让位，不加载任何自有条目
    // （与 LeviLauncher `native/levilauncher/src/config/mod_loader.cpp:42` 的约定一致）。
    if dir.join(contract::PRELOADER_MARKER).is_file() {
        log::warn!(
            "[home/preload] 实例 `{}` 存在 {}，原生模组加载交给外部预加载器",
            name,
            contract::PRELOADER_MARKER
        );
        return Ok(Probe {
            summary: PreloadSummary {
                entry_count: 0,
                loader_name: None,
                external_preloader: true,
            },
            reason: REASON_EXTERNAL,
            entries: Vec::new(),
        });
    }

    let mut entries: Vec<PreloadEntry> = Vec::new();
    let mut loader_name: Option<String> = None;
    // 模组清单读不出来不是致命问题：只影响预加载，不该让游戏启动不了。
    match super::mods::list_mods(kernel, name) {
        Ok(list) => {
            for item in list.mods.iter().filter(|item| item.enabled) {
                if item.mod_type.eq_ignore_ascii_case("preload-native") && !item.entry.is_empty()
                {
                    match relative_entry(&dir, &item.entry, &item.folder) {
                        Some(relative) => entries.push(PreloadEntry {
                            path: relative,
                            source: format!("mods/{}", item.name),
                        }),
                        None => log::warn!(
                            "[home/preload] 模组 `{}` 的 entry `{}` 逃逸出实例目录，已忽略",
                            item.name,
                            item.entry
                        ),
                    }
                }
                if item.name.to_ascii_lowercase() == LEVI_LAMINA {
                    loader_name = Some(item.name.clone());
                }
            }
        }
        Err(error) => log::warn!("[home/preload] 读取模组清单失败（不影响启动）: {error}"),
    }

    // 兜底：lipd 未按预期把加载器落成 `mods/` 结构时，实例根目录下的本体 dll
    // 仍然要能被加载，否则用户看到的是「装了加载器却没反应」。
    if loader_name.is_some() {
        for candidate in ["LeviLamina.dll", "ScriptEngine.dll"] {
            if dir.join(candidate).is_file()
                && !entries.iter().any(|entry| entry.path == candidate)
            {
                entries.push(PreloadEntry {
                    path: candidate.to_string(),
                    source: "fallback".to_string(),
                });
            }
        }
        if entries.is_empty() {
            log::warn!("[home/preload] 实例 `{name}` 标记了加载器，但没找到可加载的原生 DLL");
        }
    }

    let reason = if entries.is_empty() {
        REASON_NO_LOADER
    } else {
        REASON_AUTO
    };
    Ok(Probe {
        summary: PreloadSummary {
            entry_count: entries.len(),
            loader_name,
            external_preloader: false,
        },
        reason,
        entries,
    })
}

/// 把模组 `entry` 规整成相对实例目录、用 `/` 分隔的路径。
///
/// 绝对路径、`..`、以及解析后落到实例目录之外的路径一律拒绝：注入侧虽然也会
/// 再校验一次，但**生成侧就拒掉**能让「清单里为什么少了条目」有明确答案。
fn relative_entry(version_dir: &Path, entry: &str, folder: &str) -> Option<String> {
    let candidate = Path::new(entry);
    if candidate.is_absolute() {
        return None;
    }
    let joined = version_dir.join(folder).join(candidate);
    let normalized = normalize(&joined);
    let root = normalize(version_dir);
    let relative = normalized.strip_prefix(&root).ok()?;
    let text = relative.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        return None;
    }
    Some(text)
}

/// 词法规范化（不触碰文件系统，遇到 `..` 就上跳）。
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_entries_are_dropped() {
        assert!(relative_entry(Path::new("D:/v"), "../../evil.dll", "m").is_none());
        assert!(relative_entry(Path::new("D:/v"), "D:/other/x.dll", "m").is_none());
        assert_eq!(
            relative_entry(Path::new("D:/v"), "a.dll", "m").as_deref(),
            Some("m/a.dll")
        );
        assert_eq!(
            relative_entry(Path::new("D:/v"), "sub/a.dll", "m").as_deref(),
            Some("m/sub/a.dll")
        );
    }

    #[test]
    fn normalize_collapses_parent_segments() {
        assert_eq!(
            normalize(Path::new("D:/v/m/../m/a.dll")),
            PathBuf::from("D:/v/m/a.dll")
        );
    }
}
