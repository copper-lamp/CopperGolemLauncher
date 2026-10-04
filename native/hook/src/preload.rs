//! 按清单加载原生预加载条目（仅 Windows）。
//!

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::log;
use crate::manifest::PreloadManifest;
use crate::paths::{resolve_entry_path, Channel, RedirectPlan};
use crate::sys;

/// 单进程只执行一次预加载。
///
/// 没有这条的话，任何一次重复 `LoadLibrary` 都会让第三方加载器的全局状态初始化
/// 跑第二遍 —— 而它们的 `DllMain` 通常不幂等，表现是启动期随机崩溃。
static PRELOAD_DONE: AtomicBool = AtomicBool::new(false);

/// 一次预加载的结果（供日志与后续诊断）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PreloadOutcome {
    /// 成功加载的绝对路径。
    pub loaded: Vec<String>,
    /// 因越界 / 文件不存在被跳过的条目。
    pub skipped: Vec<String>,
    /// 外部预加载器接管（让位）。
    pub deferred_to_external: bool,
}

/// 执行预加载清单。
///
/// * 返回 `None` 表示已执行过一次，本次直接跳过（不重复加载）。
/// * 单条失败不阻断其余条目：加载器之间本就是独立关系，一个坏掉不该让
///   其它加载器也不生效。
pub fn run(plan: &RedirectPlan, manifest: &PreloadManifest) -> Option<PreloadOutcome> {
    if PRELOAD_DONE.swap(true, Ordering::AcqRel) {
        log::info("预加载已在本进程执行过，跳过");
        return None;
    }
    let mut outcome = PreloadOutcome::default();

    if crate::paths::has_external_preloader(&plan.root) {
        outcome.deferred_to_external = true;
        log::info(format!(
            "检测到 `{}`，由外部预加载器接管 mods 加载，本 DLL 不加载任何原生模组",
            crate::contract::PRELOADER_MARKER
        ));
        return Some(outcome);
    }

    log::info(format!(
        "预加载清单 reason={} 条目数={}",
        manifest.reason,
        manifest.entries.len()
    ));
    for entry in &manifest.entries {
        let Some(path) = resolve_entry_path(&plan.root, &entry.path) else {
            log::warn(format!(
                "清单条目越界或形态非法，已跳过: {:?}（source={}）",
                entry.path, entry.source
            ));
            outcome.skipped.push(entry.path.clone());
            continue;
        };
        if !path.is_file() {
            log::warn(format!(
                "清单条目不存在，已跳过: {}（source={}）",
                path.display(),
                entry.source
            ));
            outcome.skipped.push(entry.path.clone());
            continue;
        }
        match load_library(&path) {
            Ok(()) => {
                log::info(format!(
                    "已加载 {}（source={}）",
                    path.display(),
                    entry.source
                ));
                outcome.loaded.push(path.display().to_string());
            }
            Err(error) => {
                log::error(format!(
                    "加载失败 {}（source={}）: {}",
                    path.display(),
                    entry.source,
                    error
                ));
                outcome.skipped.push(entry.path.clone());
            }
        }
    }
    log::info(format!(
        "预加载结束：成功 {} 条，跳过 {} 条",
        outcome.loaded.len(),
        outcome.skipped.len()
    ));
    Some(outcome)
}

/// 只跑一次（供测试与诊断入口使用）。
pub fn reset_for_test() {
    PRELOAD_DONE.store(false, Ordering::SeqCst);
}

/// `LoadLibraryExW(LOAD_WITH_ALTERED_SEARCH_PATH)`。
fn load_library(path: &Path) -> Result<(), i32> {
    let wide = crate::sys::to_wide_nul(&path.to_string_lossy());
    // SAFETY: `wide` 以 NUL 结尾且在调用期间存活；`h_file` 传 NULL 表示不用
    // 预加载的句柄；标志位只影响搜索路径。返回 NULL 时按 `GetLastError` 判定。
    let handle = unsafe { sys::LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), sys::LOAD_WITH_ALTERED_SEARCH_PATH) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    }
    Ok(())
}

/// 诊断用：本次会话是否已经跑过预加载。
pub fn already_ran() -> bool {
    PRELOAD_DONE.load(Ordering::Acquire)
}

/// 供日志打印的渠道名（与重定向方案无关，仅诊断）。
pub fn channel_label(channel: Channel) -> &'static str {
    match channel {
        Channel::Release => "release",
        Channel::Preview => "preview",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_defaults_are_empty_and_not_deferred() {
        let outcome = PreloadOutcome::default();
        assert!(outcome.loaded.is_empty());
        assert!(outcome.skipped.is_empty());
        assert!(!outcome.deferred_to_external);
    }

    #[test]
    fn channel_labels_are_stable_strings() {
        assert_eq!(channel_label(Channel::Release), "release");
        assert_eq!(channel_label(Channel::Preview), "preview");
    }
}
