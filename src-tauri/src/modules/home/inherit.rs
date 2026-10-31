//! 实例登录态继承：只从一手来源取，绝不搬运路径绑定文件。
//!
//! # 为什么不能从 LeviLauncher 的实例继承
//!
//! LL 自己的实例数据也是从别处（官方安装 / 更早的实例）继承来的。再从 LL 继承，
//! 拿到的是 copy-of-a-copy；而路径绑定实验已经证明：.ent 授权、premium_cache、
//! XblProfileCache 这类文件只在生成它们的安装路径/授权上下文里有效。把它们搬进
//! 新路径，游戏会以为“已有授权”，反而不去重新获取 token，表现就是 0 条 TCP、
//! 永远停在“正在用 Microsoft 帐户登录”。
//!
//! # 继承策略
//!
//! | 来源 | 允许继承的内容 |
//! |---|---|
//! | 官方 Store 的 %APPDATA%\Minecraft Bedrock[ Preview] | 整份 options.txt（设置 + 三键）+ profile 骨架 |
//! | 同版本根下、没有继承标记的实例 | 只合并 options.txt 的三行登录键 |
//! | 带 instance: 标记的实例（复制品） | 跳过 |
//! | 任何 LeviLauncher 目录 | 不读、不列、不复制 |
//!
//! 明确不搬：2552228806.ent、premium_cache/、XblProfileCache/、minecraftpe/ 缓存、
//! bootstrapStorage/、Flighting/、treatments/、logs/、temp/、crash/、LocalAppData/。
//! 这些要么是路径绑定的授权/缓存，要么是每次启动重建的临时数据。
//!
//! 每次继承会在目标数据目录写 .copper-login-source，记录来源类型与路径；
//! 带 instance: 标记的实例不会再被当作来源。

use std::path::{Path, PathBuf};

use crate::error::KernelError;

use super::{isolate, login_state, meta::VersionMeta};

/// 来源标记文件名（放在数据目录根下，不参与游戏读取）。
pub const LOGIN_SOURCE_MARKER: &str = ".copper-login-source";

/// 继承方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InheritMode {
    /// 官方 Store 数据：整份 options.txt（设置 + 登录键）。
    OfficialProfile,
    /// 同版本根实例：只合并三行登录键。
    AccountKeys,
}

/// 一次继承的结果。
#[derive(Debug, Clone)]
pub struct InheritReport {
    pub source: PathBuf,
    pub target: PathBuf,
    pub mode: InheritMode,
    pub files: u64,
    pub bytes: u64,
}

/// 目标数据目录里是否已经有完整登录态。
pub fn already_signed_in(data_dir: &Path) -> bool {
    login_state::find_signed_in_user(data_dir).is_some()
}

/// 为实例找来源并继承；目标已有完整登录态或找不到来源时返回 Ok(None)。
pub fn inherit_if_needed(
    version_dir: &Path,
    meta: &VersionMeta,
    versions_root: &Path,
) -> Result<Option<InheritReport>, KernelError> {
    let target = isolate::data_dir(version_dir, meta);
    if already_signed_in(&target) {
        return Ok(None);
    }

    // 1) 一手来源：官方 Store 安装的数据目录。
    for source in official_candidates(meta) {
        if !source.is_dir() || !already_signed_in(&source) {
            continue;
        }
        let report = inherit_official_profile(&source, &target)?;
        log::info!(
            "[home/inherit] 已从官方安装 {} 继承 profile 到 {}（{} 字节）",
            report.source.display(),
            report.target.display(),
            report.bytes
        );
        return Ok(Some(report));
    }

    // 2) 次选：同版本根下、没有继承标记的实例。只搬三行登录键。
    for source in sibling_candidates(versions_root, &target) {
        if !already_signed_in(&source) {
            continue;
        }
        let report = inherit_account_keys(&source, &target)?;
        log::info!(
            "[home/inherit] 已从实例 {} 合并登录键到 {}",
            report.source.display(),
            report.target.display()
        );
        return Ok(Some(report));
    }

    Ok(None)
}

/// 官方 Store 数据目录（Release / Preview）。这是一手来源。
fn official_candidates(meta: &VersionMeta) -> Vec<PathBuf> {
    let Ok(appdata) = std::env::var("APPDATA") else {
        return Vec::new();
    };
    let name = if isolate::is_preview(meta) {
        isolate::preview_data_dir_name()
    } else {
        isolate::release_data_dir_name()
    };
    vec![Path::new(&appdata).join(name)]
}

/// 同版本根下的其它实例（排除目标、排除带 instance: 标记的复制品）。
fn sibling_candidates(versions_root: &Path, target: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(versions_root) else {
        return out;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        let Some(meta) = VersionMeta::read(&dir) else {
            continue;
        };
        let data = isolate::data_dir(&dir, &meta);
        if data == target || !data.is_dir() {
            continue;
        }
        if marker_kind(&data).as_deref() == Some("instance") {
            // 复制品不再作为来源，避免 copy-of-a-copy。
            continue;
        }
        out.push(data);
    }
    out
}

/// 读来源标记的类型（official / instance）。
fn marker_kind(data_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(data_dir.join(LOGIN_SOURCE_MARKER)).ok()?;
    text.split(':').next().map(|value| value.trim().to_string())
}

fn write_marker(target: &Path, kind: &str, source: &Path) -> Result<(), KernelError> {
    std::fs::create_dir_all(target)?;
    let text = format!("{kind}:{}\n", source.display());
    std::fs::write(target.join(LOGIN_SOURCE_MARKER), text.as_bytes())?;
    Ok(())
}

/// 官方来源：整份 options.txt + profile 骨架。
fn inherit_official_profile(source: &Path, target: &Path) -> Result<InheritReport, KernelError> {
    let Some((xuid, state)) = login_state::find_signed_in_user(source) else {
        return Err(KernelError::InvalidArgument(
            "官方数据目录里没有完整登录态".into(),
        ));
    };
    let source_options = login_state::profile_options_path(source, &xuid);
    let target_options = login_state::profile_options_path(target, &xuid);
    let mut files = 0;
    let mut bytes = 0;
    if target_options.is_file() {
        // 目标已有设置：只合并三键，保留用户自己的画质/按键。
        if login_state::merge_account_into(&target_options, &state)? {
            files = 1;
            bytes = std::fs::metadata(&target_options)
                .map(|meta| meta.len())
                .unwrap_or(0);
        }
    } else {
        if let Some(parent) = target_options.parent() {
            std::fs::create_dir_all(parent)?;
        }
        bytes = std::fs::copy(&source_options, &target_options)?;
        files = 1;
    }
    write_marker(target, "official", source)?;
    if !already_signed_in(target) {
        return Err(KernelError::InvalidArgument(
            "继承后目标仍没有完整登录态".into(),
        ));
    }
    Ok(InheritReport {
        source: source.to_path_buf(),
        target: target.to_path_buf(),
        mode: InheritMode::OfficialProfile,
        files,
        bytes,
    })
}

/// 同版本根实例：只合并三行登录键，绝不复制授权/缓存文件。
fn inherit_account_keys(source: &Path, target: &Path) -> Result<InheritReport, KernelError> {
    let Some((xuid, state)) = login_state::find_signed_in_user(source) else {
        return Err(KernelError::InvalidArgument(
            "来源实例里没有完整登录态".into(),
        ));
    };
    let target_options = login_state::profile_options_path(target, &xuid);
    let changed = login_state::merge_account_into(&target_options, &state)?;
    write_marker(target, "instance", source)?;
    if !already_signed_in(target) {
        return Err(KernelError::InvalidArgument(
            "合并登录键后目标仍没有完整登录态".into(),
        ));
    }
    let bytes = std::fs::metadata(&target_options)
        .map(|meta| meta.len())
        .unwrap_or(0);
    Ok(InheritReport {
        source: source.to_path_buf(),
        target: target.to_path_buf(),
        mode: InheritMode::AccountKeys,
        files: u64::from(changed),
        bytes: if changed { bytes } else { 0 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "copper_inherit_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_profile(root: &Path, options: &str) {
        let path = root.join("Users/123/games/com.mojang/minecraftpe/options.txt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, options).unwrap();
    }

    const SIGNED_IN: &str =
        "game_haseverloggedintoxbl:1\nlast_minecraft_id:ABC\nlast_title_account_id:DEF\n";

    #[test]
    fn official_copy_takes_options_but_not_licenses_or_caches() {
        let source = temp_dir("official-src");
        let target = temp_dir("official-dst");
        write_profile(&source, &format!("gfx_field_of_view:30\n{SIGNED_IN}"));
        std::fs::write(source.join("2552228806.ent"), b"license").unwrap();
        std::fs::create_dir_all(source.join("premium_cache")).unwrap();
        std::fs::write(source.join("premium_cache/cache.bin"), b"cache").unwrap();
        std::fs::create_dir_all(source.join("minecraftpe/XblProfileCache")).unwrap();
        std::fs::write(source.join("minecraftpe/XblProfileCache/p.bin"), b"profile").unwrap();

        let report = inherit_official_profile(&source, &target).unwrap();
        assert_eq!(report.mode, InheritMode::OfficialProfile);
        let copied = std::fs::read_to_string(
            target.join("Users/123/games/com.mojang/minecraftpe/options.txt"),
        )
        .unwrap();
        assert!(copied.contains("gfx_field_of_view:30"));
        assert!(copied.contains("last_minecraft_id:ABC"));
        assert!(!target.join("2552228806.ent").exists());
        assert!(!target.join("premium_cache").exists());
        assert!(!target.join("minecraftpe/XblProfileCache").exists());
        assert!(target.join(LOGIN_SOURCE_MARKER).is_file());
    }

    #[test]
    fn account_keys_merge_without_touching_settings() {
        let source = temp_dir("keys-src");
        let target = temp_dir("keys-dst");
        write_profile(&source, SIGNED_IN);
        write_profile(&target, "gfx_field_of_view:30\n");
        std::fs::write(source.join("2552228806.ent"), b"license").unwrap();

        let report = inherit_account_keys(&source, &target).unwrap();
        assert_eq!(report.mode, InheritMode::AccountKeys);
        let text = std::fs::read_to_string(
            target.join("Users/123/games/com.mojang/minecraftpe/options.txt"),
        )
        .unwrap();
        assert!(text.contains("gfx_field_of_view:30"));
        assert!(text.contains("game_haseverloggedintoxbl:1"));
        assert!(!target.join("2552228806.ent").exists());
        assert!(target.join(LOGIN_SOURCE_MARKER).is_file());
    }

    #[test]
    fn inherited_marker_is_recognized() {
        let dir = temp_dir("marker");
        write_marker(&dir, "instance", Path::new("D:/x")).unwrap();
        assert_eq!(marker_kind(&dir).as_deref(), Some("instance"));
        write_marker(&dir, "official", Path::new("D:/y")).unwrap();
        assert_eq!(marker_kind(&dir).as_deref(), Some("official"));
    }

    #[test]
    fn sibling_sources_skip_inherited_instances() {
        let versions = temp_dir("siblings");
        let target_dir = versions.join("Target");
        let direct_dir = versions.join("Direct");
        let copy_dir = versions.join("Copy");
        for dir in [&target_dir, &direct_dir, &copy_dir] {
            std::fs::create_dir_all(dir).unwrap();
            let meta = VersionMeta::default();
            std::fs::write(
                dir.join("version.json"),
                serde_json::to_string(&meta).unwrap(),
            )
            .unwrap();
        }
        let target = isolate::data_dir(&target_dir, &VersionMeta::default());
        write_profile(&isolate::data_dir(&direct_dir, &VersionMeta::default()), SIGNED_IN);
        let copy_data = isolate::data_dir(&copy_dir, &VersionMeta::default());
        write_profile(&copy_data, SIGNED_IN);
        write_marker(&copy_data, "instance", &copy_dir).unwrap();

        let sources = sibling_candidates(&versions, &target);
        assert!(sources.contains(&isolate::data_dir(&direct_dir, &VersionMeta::default())));
        assert!(!sources.contains(&copy_data));
    }

    #[test]
    fn end_to_end_sibling_copies_keys_but_no_license() {
        let versions = temp_dir("e2e");
        let target_dir = versions.join("Target");
        let source_dir = versions.join("Source");
        for dir in [&target_dir, &source_dir] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join("version.json"),
                serde_json::to_string(&VersionMeta::default()).unwrap(),
            )
            .unwrap();
        }
        let source_data = isolate::data_dir(&source_dir, &VersionMeta::default());
        write_profile(&source_data, SIGNED_IN);
        std::fs::write(source_data.join("2552228806.ent"), b"license").unwrap();
        let target_data = isolate::data_dir(&target_dir, &VersionMeta::default());

        let report = inherit_if_needed(&target_dir, &VersionMeta::default(), &versions)
            .unwrap()
            .expect("应当有一个可用的来源");
        assert!(report.files <= 1);
        let text = std::fs::read_to_string(
            login_state::profile_options_path(&target_data, "123"),
        )
        .unwrap();
        assert!(text.contains("game_haseverloggedintoxbl:1"));
        assert!(text.contains("last_minecraft_id:ABC"));
        assert!(!target_data.join("2552228806.ent").exists());
        assert!(target_data.join(LOGIN_SOURCE_MARKER).is_file());
    }

    #[test]
    fn skips_when_target_already_signed_in() {
        let versions = temp_dir("signed");
        let dir = versions.join("Demo");
        std::fs::create_dir_all(&dir).unwrap();
        let meta = VersionMeta::default();
        std::fs::write(
            dir.join("version.json"),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();
        write_profile(&isolate::data_dir(&dir, &meta), SIGNED_IN);

        let result = inherit_if_needed(&dir, &meta, &versions).unwrap();
        assert!(result.is_none());
    }
}
