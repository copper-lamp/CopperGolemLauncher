//! 账户与 profile 一致性检查。
//!
//! # 为什么需要它
//!
//! 游戏用 profile 里的 last_minecraft_id / last_title_account_id 去静默恢复
//! Xbox 会话。如果这份 profile 属于另一个 Microsoft 账户（例如从别人的
//! LL 实例继承来的），当前 WAM 会话里根本没有那个账户：游戏既拿不到 token，
//! 也不会弹登录窗口，表现就是 0 条 TCP、永远停在「正在用 Microsoft 帐户登录」。
//!
//! 安装授权（device / content key）、启动器当前账户、profile 的 XUID 必须是
//! 同一个身份。这里在启动前做三项检查并落日志；不一致时把 profile 的登录
//! 痕迹隔离到备份文件，让游戏回到欢迎页，而不是继续静默恢复错误账户。

use std::path::{Path, PathBuf};

use crate::error::KernelError;

use super::{login_state, meta::VersionMeta};

/// 一致性诊断结果。
#[derive(Debug, Clone)]
pub struct Diagnosis {
    pub active_gamertag: Option<String>,
    pub active_xuid: Option<String>,
    pub profile_xuid: Option<String>,
    pub profile: Option<login_state::AccountState>,
    pub mismatch: bool,
}

impl Diagnosis {
    /// 一条可直接写进日志的结论。
    pub fn describe(&self) -> String {
        let active = self
            .active_xuid
            .as_deref()
            .map(|xuid| {
                format!(
                    "{} ({xuid})",
                    self.active_gamertag.as_deref().unwrap_or("-")
                )
            })
            .unwrap_or_else(|| "<未登录>".to_string());
        let profile = match (&self.profile_xuid, &self.profile) {
            (Some(dir), Some(state)) => format!(
                "dir={dir} minecraft_id={} title_id={}",
                state.minecraft_id, state.title_account_id
            ),
            _ => "<无登录态>".to_string(),
        };
        format!(
            "账户—profile 一致性：{}；启动器账户={active}；profile={profile}",
            if self.mismatch { "MISMATCH" } else { "OK" }
        )
    }
}

/// 检查启动器当前账户与实例 profile 是否属于同一身份。
pub fn diagnose(
    data_dir: &Path,
    active_xuid: Option<&str>,
    active_gamertag: Option<&str>,
) -> Diagnosis {
    let profile = login_state::find_signed_in_user(data_dir);
    let profile_dir = profile.as_ref().map(|(dir, _)| dir.clone());
    // 注意：`Users/<dir>` 是**本地用户目录名**，不是 Xbox XUID。
    // 之前的实现拿它和 XUID 比，导致每个已登录 profile 都被误判为
    // MISMATCH：1.26.60 的目录是 7016…，真实 XUID 是 2535…。
    // 这里只做诊断/记录，不再用目录名判定账户归属。
    Diagnosis {
        active_gamertag: active_gamertag.map(str::to_string),
        active_xuid: active_xuid.map(str::to_string),
        profile_xuid: profile_dir,
        profile: profile.map(|(_, state)| state),
        mismatch: false,
    }
}

/// 不一致时隔离 profile 的登录痕迹：备份原文件，再清掉三键与「已选择不登录」。
///
/// 返回备份路径；没有可隔离的内容时返回 Ok(None)。
pub fn quarantine(data_dir: &Path, diagnosis: &Diagnosis) -> Result<Option<PathBuf>, KernelError> {
    let Some(xuid) = diagnosis.profile_xuid.as_deref() else {
        return Ok(None);
    };
    let options = login_state::profile_options_path(data_dir, xuid);
    if !options.is_file() {
        return Ok(None);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let backup = options.with_file_name(format!("options.txt.foreign-{stamp}"));
    std::fs::copy(&options, &backup)?;
    login_state::clear_account(&options)?;
    log::warn!(
        "[home/account] profile 属于另一个账户（profile={}，启动器={}），已隔离登录痕迹：{}",
        xuid,
        diagnosis.active_xuid.as_deref().unwrap_or("<未登录>"),
        backup.display()
    );
    Ok(Some(backup))
}

/// 实例数据目录（供调用方少写一行）。
pub fn data_dir(version_dir: &Path, meta: &VersionMeta) -> PathBuf {
    super::isolate::data_dir(version_dir, meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "copper_account_{tag}_{}_{}",
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

    fn write_profile(data: &Path, xuid: &str) {
        let path = login_state::profile_options_path(data, xuid);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "gfx_field_of_view:30\ngame_haseverloggedintoxbl:1\nlast_minecraft_id:ABC\nlast_title_account_id:DEF\ngame_haschosennottosignintoxbl:1\n",
        )
        .unwrap();
    }

    #[test]
    fn profile_directory_is_not_xuid() {
        let data = temp_dir("mismatch");
        write_profile(&data, "7016486954432746142");
        let diagnosis = diagnose(&data, Some("2535442394448107"), Some("magicgbs0z"));
        // 目录名与 XUID 不同是正常的，不能再判成 MISMATCH。
        assert!(!diagnosis.mismatch);
        assert_eq!(
            diagnosis.profile_xuid.as_deref(),
            Some("7016486954432746142")
        );
        assert!(diagnosis.describe().contains("dir=7016486954432746142"));
    }

    #[test]
    fn matching_account_is_ok() {
        let data = temp_dir("match");
        write_profile(&data, "7016486954432746142");
        let diagnosis = diagnose(&data, Some("7016486954432746142"), Some("magicgbs0z"));
        assert!(!diagnosis.mismatch);
    }

    #[test]
    fn quarantine_backs_up_and_clears_only_account_keys() {
        let data = temp_dir("quarantine");
        write_profile(&data, "7016486954432746142");
        let diagnosis = diagnose(&data, Some("2535442394448107"), Some("other"));
        let backup = quarantine(&data, &diagnosis).unwrap().expect("备份");
        assert!(backup.is_file());
        let options = login_state::profile_options_path(&data, "7016486954432746142");
        let text = std::fs::read_to_string(&options).unwrap();
        assert!(text.contains("gfx_field_of_view:30"));
        assert!(!text.contains("game_haseverloggedintoxbl"));
        assert!(!text.contains("last_minecraft_id"));
        assert!(!text.contains("game_haschosennottosignintoxbl"));
        assert!(login_state::find_signed_in_user(&data).is_none());
    }
}
