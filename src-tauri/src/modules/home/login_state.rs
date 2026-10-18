//! 登录态导入：让新实例「第一次启动就是已登录」。
//!
//! # 为什么需要这个模块
//!
//! 隔离实例的 profile 是全新的。全新 profile 上游戏的**交互式登录走不通**：
//! 点「立即登录」后界面切到「登录中」，但全程零 TCP 连接（实测 12 分钟无一次
//! 网络请求），永远停在那里。这不是隔离层的锅——完全不注入、让游戏用真实
//! `%APPDATA%` 时表现一模一样（见 `docs/启动链路与实例隔离.md` §4.2）。
//!
//! 而 profile 里一旦带着「本机登录过」的痕迹，游戏就直接进主菜单、复用系统
//! Xbox 应用里的帐户，不再走那条交互式路径。
//!
//! # 那条痕迹是什么（实机二分出来的）
//!
//! 不是 token 文件，是 `Users/<xuid>/games/com.mojang/minecraftpe/options.txt`
//! 里的三行：
//!
//! ```text
//! game_haseverloggedintoxbl:1
//! last_minecraft_id:<Xbox 帐户 id>
//! last_title_account_id:<title 帐户 id>
//! ```
//!
//! 二分过程（每轮都是真机启动 + 截图判定）：
//!
//! | 组 | 内容 | 结果 |
//! |---|---|---|
//! | 空 profile | — | 欢迎登录页 |
//! | 空 + `minecraftpe/`（236MB 缓存） | 去掉 | 照样进主菜单 → 不需要 |
//! | 空 + `.ent` + `premium_cache/` | 去掉 | 照样进主菜单 → 不需要 |
//! | 空 + 整个 `Users/` | 加上 | 进主菜单 |
//! | 空 + `Users/<xuid>/games/com.mojang/minecraftpe/` | 加上 | 进主菜单 |
//! | 空 + 只写上面三行 | 加上 | 进主菜单 |
//!
//! 所以**只搬三行文本**，不碰存档、不碰缓存、不动用户的画质设置。
//!
//! # 与其它模块的关系
//!
//! - 由 `home::launch::prepare_instance` 在每次启动前调用（幂等：已有登录态就
//!   什么都不做），因此新建实例、导入的实例、以前装好的实例都能自愈；
//! - 数据目录规则复用 [`super::isolate::data_dir`]，不自己拼路径；
//! - 来源可以是同版本根下的其它实例，也可以是官方 Store 安装的
//!   `%APPDATA%\Minecraft Bedrock`——用户只要在任何一个地方登录过就够。

use std::path::{Path, PathBuf};

use crate::error::KernelError;

use super::{isolate, meta::VersionMeta};

/// 每用户选项文件名（相对 `.../com.mojang/minecraftpe/`）。
pub const OPTIONS_FILE: &str = "options.txt";
/// 「曾经登录过 XBL」开关。
pub const KEY_EVER_LOGGED_IN: &str = "game_haseverloggedintoxbl";
/// Xbox 帐户 id（profile 侧）。
pub const KEY_MC_ID: &str = "last_minecraft_id";
/// title 帐户 id（授权侧）。
pub const KEY_TITLE_ID: &str = "last_title_account_id";

/// 需要导入/判定的三个键。
pub const ACCOUNT_KEYS: [&str; 3] = [KEY_EVER_LOGGED_IN, KEY_MC_ID, KEY_TITLE_ID];

/// profile 里的帐户痕迹。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountState {
    /// `game_haseverloggedintoxbl` 是否为 `1`。
    pub ever_logged_in: bool,
    /// `last_minecraft_id` 原值。
    pub minecraft_id: String,
    /// `last_title_account_id` 原值。
    pub title_account_id: String,
}

impl AccountState {
    /// 三个键是否足以让游戏跳过登录欢迎页。
    ///
    /// 判定必须严格：只写一半（比如有 id 没有开关）在实机上仍会弹欢迎页，
    /// 那等于白导入一次还让用户以为"已经登录了"。
    pub fn is_complete(&self) -> bool {
        self.ever_logged_in && !self.minecraft_id.is_empty() && !self.title_account_id.is_empty()
    }
}

/// `options.txt` 的绝对路径。
pub fn profile_options_path(data_dir: &Path, xuid: &str) -> PathBuf {
    data_dir
        .join("Users")
        .join(xuid)
        .join("games")
        .join("com.mojang")
        .join("minecraftpe")
        .join(OPTIONS_FILE)
}

/// 解析 `options.txt` 文本里的帐户痕迹。
///
/// 格式固定为 `key:value` 一行一条；游戏自己写的键值里不含换行。无法识别的行
/// 一律忽略——这是别人的格式，读不懂只能不读，不能猜。
pub fn parse_account(text: &str) -> AccountState {
    let mut state = AccountState::default();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            KEY_EVER_LOGGED_IN => state.ever_logged_in = value.trim() == "1",
            KEY_MC_ID => state.minecraft_id = value.trim().to_string(),
            KEY_TITLE_ID => state.title_account_id = value.trim().to_string(),
            _ => {}
        }
    }
    state
}

/// 从一个 profile 的 `options.txt` 读取帐户痕迹（文件不存在返回 `None`）。
pub fn read_account(options: &Path) -> Option<AccountState> {
    let text = std::fs::read_to_string(options).ok()?;
    Some(parse_account(&text))
}

/// 该 profile 是否已经带着完整登录态。
pub fn has_signed_in(options: &Path) -> bool {
    read_account(options).map(|state| state.is_complete()).unwrap_or(false)
}

/// 把来源的帐户痕迹合并进目标的 `options.txt`。
///
/// # 只改三行
///
/// 目标文件里的画质、音频、按键等设置一律保留；只替换/追加上面那三个键。
/// 这正是二分实验得出的最小改动面——把整份 `options.txt` 拷过去会顺手覆盖用户
/// 的视距、FOV 等设置，那是没人预期的副作用。
///
/// 返回是否真的写盘（三个键原本就正确时返回 `false`，保持幂等）。
pub fn merge_account_into(target: &Path, source: &AccountState) -> Result<bool, KernelError> {
    if !source.is_complete() {
        return Err(KernelError::InvalidArgument(
            "来源 profile 的帐户痕迹不完整，拒绝导入".into(),
        ));
    }
    let existing = std::fs::read_to_string(target).unwrap_or_default();
    if parse_account(&existing) == *source && target.is_file() {
        return Ok(false);
    }

    let wanted = [
        (KEY_EVER_LOGGED_IN, "1".to_string()),
        (KEY_MC_ID, source.minecraft_id.clone()),
        (KEY_TITLE_ID, source.title_account_id.clone()),
    ];

    let mut out: Vec<String> = Vec::new();
    let mut written = [false; 3];
    for line in existing.lines() {
        let bare = line.trim_end_matches('\r');
        let key = bare.split_once(':').map(|(key, _)| key.trim()).unwrap_or("");
        match wanted.iter().position(|(name, _)| *name == key) {
            Some(index) if !written[index] => {
                out.push(format!("{}:{}", wanted[index].0, wanted[index].1));
                written[index] = true;
            }
            // 重复键只保留第一份，与游戏自身的读取顺序一致。
            Some(_) => {}
            None => out.push(bare.to_string()),
        }
    }
    for (index, (name, value)) in wanted.iter().enumerate() {
        if !written[index] {
            out.push(format!("{name}:{value}"));
        }
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 先写临时文件再改名：中途崩溃不会留下半个 options.txt，而半个文件会被
    // 游戏当成"设置损坏"重置，用户会莫名丢掉全部画质设置。
    let tmp = target.with_extension("copper-tmp");
    let mut text = out.join("\r\n");
    text.push_str("\r\n");
    std::fs::write(&tmp, text.as_bytes())?;
    std::fs::rename(&tmp, target)?;
    Ok(true)
}

/// 在一个数据目录里找出带完整登录态的 xuid 目录。
pub fn find_signed_in_user(data_dir: &Path) -> Option<(String, AccountState)> {
    let users = data_dir.join("Users");
    let entries = std::fs::read_dir(&users).ok()?;
    for entry in entries.flatten() {
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let xuid = entry.file_name().to_string_lossy().into_owned();
        // `Shared` 是公共内容目录，不是玩家。
        if xuid.eq_ignore_ascii_case("Shared") {
            continue;
        }
        let options = profile_options_path(data_dir, &xuid);
        if let Some(state) = read_account(&options) {
            if state.is_complete() {
                return Some((xuid, state));
            }
        }
    }
    None
}

/// 一次导入的结果（供日志与上层展示）。
#[derive(Debug, Clone)]
pub struct ImportOutcome {
    /// 来源数据目录。
    pub source: PathBuf,
    /// 目标数据目录。
    pub target: PathBuf,
    /// 目标里被写入的 xuid 目录名。
    pub xuid: String,
    /// 是否真的写了盘。
    pub changed: bool,
}

/// 把 `source_data_dir` 的登录态搬到 `target_data_dir`。
///
/// 目标已有完整登录态时返回 `Ok(None)`：**不覆盖**。用户在这个实例里自己登录过
/// 就该保留他自己的帐户，不能被导入逻辑顶掉。
pub fn import_from(
    source_data_dir: &Path,
    target_data_dir: &Path,
) -> Result<Option<ImportOutcome>, KernelError> {
    if find_signed_in_user(target_data_dir).is_some() {
        return Ok(None);
    }
    let Some((xuid, state)) = find_signed_in_user(source_data_dir) else {
        return Ok(None);
    };
    // 来源与目标是同一个目录时，`find_signed_in_user` 早就命中了；这里再兜一次，
    // 避免把用户刚写的文件当成"旧的三行"重写成同一份内容。
    if source_data_dir == target_data_dir {
        return Ok(None);
    }
    let target_options = profile_options_path(target_data_dir, &xuid);
    let changed = merge_account_into(&target_options, &state)?;
    Ok(Some(ImportOutcome {
        source: source_data_dir.to_path_buf(),
        target: target_data_dir.to_path_buf(),
        xuid,
        changed,
    }))
}

/// 候选来源：同版本根下的其它实例 + 官方 Store 安装的数据目录。
///
/// 顺序即优先级：同版本根下的实例最贴切（同一套隔离布局），官方安装次之
/// （用户可能只在商店版里登录过）。`exclude` 用来排除目标自己。
pub fn source_candidates(versions_root: &Path, exclude: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(versions_root) {
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
            if data != exclude && !out.contains(&data) {
                out.push(data);
            }
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        for name in [isolate::release_data_dir_name(), isolate::preview_data_dir_name()] {
            let path = Path::new(&appdata).join(name);
            if path != exclude && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// 为某个实例数据目录找来源并导入（幂等；找不到来源返回 `Ok(None)`）。
pub fn import_for_instance(
    versions_root: &Path,
    data_dir: &Path,
) -> Result<Option<ImportOutcome>, KernelError> {
    if find_signed_in_user(data_dir).is_some() {
        return Ok(None);
    }
    for candidate in source_candidates(versions_root, data_dir) {
        match import_from(&candidate, data_dir)? {
            Some(outcome) => return Ok(Some(outcome)),
            None => continue,
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("copper_login_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 在临时目录里按真实布局写一个 profile 的 `options.txt`。
    fn seed_options(data_dir: &Path, xuid: &str, text: &str) {
        let path = profile_options_path(data_dir, xuid);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }

    #[test]
    fn parses_account_keys_only() {
        let state = parse_account(
            "gfx_viewdistance:640\r\ngame_haseverloggedintoxbl:1\r\nlast_minecraft_id:ABC\r\nlast_title_account_id:DEF\r\n",
        );
        assert!(state.is_complete());
        assert_eq!(state.minecraft_id, "ABC");
        assert_eq!(state.title_account_id, "DEF");
    }

    #[test]
    fn incomplete_state_is_rejected() {
        // 只有 id 没有开关：实机上仍然会弹欢迎页，所以不算登录态。
        let state = parse_account("last_minecraft_id:ABC\nlast_title_account_id:DEF\n");
        assert!(!state.is_complete());
        let only_flag = parse_account("game_haseverloggedintoxbl:1\n");
        assert!(!only_flag.is_complete());
    }

    #[test]
    fn merge_preserves_unrelated_settings() {
        let dir = temp_dir("merge");
        let options = dir.join(OPTIONS_FILE);
        std::fs::write(&options, "gfx_field_of_view:79\ngame_haseverloggedintoxbl:0\n").unwrap();

        let source = AccountState {
            ever_logged_in: true,
            minecraft_id: "ABC".into(),
            title_account_id: "DEF".into(),
        };
        assert!(merge_account_into(&options, &source).unwrap());

        let text = std::fs::read_to_string(&options).unwrap();
        assert!(text.contains("gfx_field_of_view:79"), "用户的画质设置不能被动");
        assert!(text.contains("game_haseverloggedintoxbl:1"));
        assert!(text.contains("last_minecraft_id:ABC"));
        assert!(text.contains("last_title_account_id:DEF"));
        // 幂等：第二次不再写盘。
        assert!(!merge_account_into(&options, &source).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_refuses_incomplete_source() {
        let dir = temp_dir("refuse");
        let options = dir.join(OPTIONS_FILE);
        let bad = AccountState {
            ever_logged_in: true,
            minecraft_id: String::new(),
            title_account_id: "DEF".into(),
        };
        assert!(merge_account_into(&options, &bad).is_err());
        assert!(!options.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_creates_parent_directories() {
        let dir = temp_dir("create");
        let target = profile_options_path(&dir, "123");
        let source = AccountState {
            ever_logged_in: true,
            minecraft_id: "ABC".into(),
            title_account_id: "DEF".into(),
        };
        assert!(merge_account_into(&target, &source).unwrap());
        assert!(target.is_file());
        assert!(target.ends_with(Path::new("minecraftpe").join(OPTIONS_FILE)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_signed_in_user_skips_shared_and_incomplete() {
        let dir = temp_dir("find");
        seed_options(&dir, "Shared", "game_haseverloggedintoxbl:1\n");
        seed_options(&dir, "111", "last_minecraft_id:ABC\n");
        assert!(find_signed_in_user(&dir).is_none());

        seed_options(
            &dir,
            "222",
            "game_haseverloggedintoxbl:1\nlast_minecraft_id:ABC\nlast_title_account_id:DEF\n",
        );
        let (xuid, state) = find_signed_in_user(&dir).unwrap();
        assert_eq!(xuid, "222");
        assert!(state.is_complete());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_does_not_overwrite_existing_login() {
        let source = temp_dir("src");
        let target = temp_dir("dst");
        seed_options(
            &source,
            "111",
            "game_haseverloggedintoxbl:1\nlast_minecraft_id:ABC\nlast_title_account_id:DEF\n",
        );
        seed_options(
            &target,
            "999",
            "game_haseverloggedintoxbl:1\nlast_minecraft_id:OTHER\nlast_title_account_id:OTHER2\n",
        );

        assert!(import_from(&source, &target).unwrap().is_none());
        let text = std::fs::read_to_string(profile_options_path(&target, "999")).unwrap();
        assert!(text.contains("last_minecraft_id:OTHER"), "用户自己的帐户不能被顶掉");
        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn import_copies_only_the_account_keys() {
        let source = temp_dir("src2");
        let target = temp_dir("dst2");
        seed_options(
            &source,
            "111",
            "gfx_field_of_view:30\ngame_haseverloggedintoxbl:1\nlast_minecraft_id:ABC\nlast_title_account_id:DEF\n",
        );
        let outcome = import_from(&source, &target).unwrap().unwrap();
        assert!(outcome.changed);
        let text = std::fs::read_to_string(profile_options_path(&target, "111")).unwrap();
        assert!(text.contains("last_minecraft_id:ABC"));
        assert!(
            !text.contains("gfx_field_of_view:30"),
            "来源的其它设置不能跟着过来"
        );
        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&target);
    }
}
