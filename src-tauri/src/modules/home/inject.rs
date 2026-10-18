//! hook DLL 的落位与启动文件维护。
//!
//! # 注入方式：远程线程，不是改导入表
//!
//! 早先的实现走的是「给游戏 exe 的导入表追加一条我们自己的 DLL」——也就是
//! [`crate::pe`] 的 `plan_add_import`。这条路在本机的真实 GDK 游戏
//! （1.26.52.03）上会被加载器以 `0xC0000135` 无声拒绝，且**不产生任何
//! Application Error 事件、也没有「缺少 DLL」对话框**：镜像结构、节表、
//! 数据目录、节内容逐字节校验全部自洽，`dumpbin` 却读不出导入表。
//! 在没有调试器逐条比对加载器内部行为之前，这条路不值得继续投入。
//!
//! 现在改为**不改游戏 exe 一个字节**：以 `CREATE_SUSPENDED` 启动游戏，
//! 在主线程尚未执行任何指令时向其地址空间写入 hook DLL 的绝对路径，再起远程线程
//! 调用 `LoadLibraryW`，加载完成后恢复主线程。实现在 [`super::injector`]。
//!
//! 因此本模块的职责收缩为：
//!
//! - 把最新版的 hook DLL 写进实例目录（远程注入需要一个稳定的绝对路径）；
//! - 控制台子系统开关（仍要改 exe 的一个字段，改完立刻可逆）；
//! - 清理**历史遗留**的导入表注入（老版本写过、现在不该再写）。
//!
//! 交接变量仍通过 [`crate::modules::home::launch`] 构造的环境块下发，
//! hook 侧读取方式不变（`COPPER_HOOK_*`）。细节与取舍见
//! `docs/启动链路与实例隔离.md`。

use std::path::{Path, PathBuf};

use copper_core_hook::contract as contract;

use crate::error::KernelError;
use crate::modules::home::meta::VersionMeta;
// `pe` 只在 Windows 分支被调用，非 Windows 目标上这两个别名都不存在。
#[cfg(windows)]
use crate::pe::{self, PeError};

/// 内核内嵌的 hook DLL 字节（由 `build.rs` 定位产物）。
#[cfg(windows)]
const HOOK_DLL_BYTES: &[u8] = include_bytes!(env!("COPPER_HOOK_DLL"));

/// 注入结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InjectOutcome {
    /// 是否新写入了 hook DLL。
    pub dll_written: bool,
    /// 是否清理掉了历史遗留的导入表注入。
    pub legacy_import_removed: bool,
}

/// 启动文件状态（版本设置页展示用）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LaunchFileState {
    pub hook_dll_present: bool,
    /// 注入是否可用：hook DLL 已就位，且启动文件上**没有**遗留的导入表注入。
    pub injected: bool,
    /// 历史备份文件是否存在（导入表注入时期的遗留物，可删除）。
    pub backup_exists: bool,
    /// 备份文件字节数（0 = 无备份）。
    pub backup_bytes: u64,
    pub console: bool,
}

#[cfg(windows)]
fn pe_err(context: &str, error: PeError) -> KernelError {
    KernelError::InvalidArgument(format!("{context}：{error}"))
}

/// hook DLL 在实例目录下的路径。
pub fn hook_dll_path(version_dir: &Path) -> PathBuf {
    version_dir.join(contract::HOOK_DLL_FILE_NAME)
}

/// 历史遗留的原始启动文件备份路径（导入表注入时期产生）。
pub fn backup_path(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_os_string();
    name.push(contract::BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// 预加载清单位置。
pub fn manifest_path(version_dir: &Path) -> PathBuf {
    version_dir.join(contract::MANIFEST_FILE_NAME)
}

/// 游戏启动文件是否仍带着我们写入的导入表注入（历史遗留）。
#[cfg(windows)]
pub fn has_legacy_import(exe: &Path) -> bool {
    std::fs::read(exe)
        .ok()
        .and_then(|bytes| pe::parse(bytes).ok())
        .map(|image| {
            image
                .dll_is_imported(contract::HOOK_DLL_FILE_NAME)
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// 非 Windows 没有导入表注入概念，恒为 false。
#[cfg(not(windows))]
fn has_legacy_import(_exe: &Path) -> bool {
    false
}

/// 查询启动文件状态（不修改任何文件）。
pub fn launch_file_state(version_dir: &Path, exe: &Path, meta: &VersionMeta) -> LaunchFileState {
    let backup = backup_path(exe);
    let hook_dll_present = hook_dll_path(version_dir).is_file();
    // 残留着导入表注入时报「不可用」：那份 exe 正是加载器拒绝启动的产物，
    // 用户在这里看到的必须是「有东西要清」，而不是「一切正常」。
    let legacy = has_legacy_import(exe);
    LaunchFileState {
        injected: hook_dll_present && !legacy,
        hook_dll_present,
        backup_exists: backup.is_file(),
        backup_bytes: std::fs::metadata(&backup).map(|m| m.len()).unwrap_or(0),
        console: meta.enable_console,
    }
}

/// 确保实例目录里有最新版本的 hook DLL，按 SHA256 比对决定是否重写。
#[cfg(windows)]
fn ensure_hook_dll(version_dir: &Path) -> Result<bool, KernelError> {
    let target = hook_dll_path(version_dir);
    if let Ok(existing) = std::fs::read(&target) {
        if sha256(&existing) == sha256(HOOK_DLL_BYTES) {
            return Ok(false);
        }
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(&target, HOOK_DLL_BYTES)?;
    Ok(true)
}

/// 非 Windows 没有 hook DLL 落位这一步。
#[cfg(not(windows))]
fn ensure_hook_dll(_version_dir: &Path) -> Result<bool, KernelError> {
    Ok(false)
}

/// 写文件（临时文件 + rename），失败时不留半成品。
#[cfg(windows)]
fn atomic_write(target: &Path, bytes: &[u8]) -> Result<(), KernelError> {
    let tmp = target.with_extension("copper-tmp");
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            Err(error.into())
        }
    }
}

/// 用 `MoveFileExW` 原子替换 exe（保留原文件 ACL）。
///
/// 为什么仍然需要它：控制台子系统开关改的是 exe 本身的一个字段。
/// 中途失败 / 断电 / 被占用时直接写会留下**半张 PE**，用户的实例就此报废；
/// `MoveFileExW + MOVEFILE_WRITE_THROUGH` 是单次原子替换。
#[cfg(windows)]
fn replace_exe(exe: &Path, bytes: &[u8]) -> Result<(), KernelError> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, MoveFileExW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_WRITE_THROUGH,
        FILE_READ_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA,
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, OPEN_EXISTING,
    };

    let tmp = exe.with_extension("copper-inject-tmp");
    if let Some(parent) = tmp.parent() {
        std::fs::create_dir_all(parent)?;
    }
    {
        use std::os::windows::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        options.custom_flags(FILE_FLAG_WRITE_THROUGH.0);
        let mut file = options.open(&tmp).map_err(|e| {
            KernelError::InvalidArgument(format!(
                "写入启动文件临时副本失败：{e}（游戏运行中时无法改写启动文件，请先退出游戏）"
            ))
        })?;
        std::io::Write::write_all(&mut file, bytes).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            KernelError::Io(e)
        })?;
    }
    let wide = |path: &Path| -> Vec<u16> {
        path.to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    };
    unsafe {
        // 先探一次可写性：游戏运行中时 exe 是独占的，此时直接报明确的错，
        // 好过让 MoveFileExW 失败后给出含糊的「替换失败」。
        let opened = CreateFileW(
            PCWSTR(wide(exe).as_ptr()),
            FILE_READ_DATA.0 | FILE_WRITE_DATA.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        );
        if opened.is_err() {
            let _ = std::fs::remove_file(&tmp);
            return Err(KernelError::InvalidArgument(
                "无法写入游戏启动文件（游戏运行中或文件被占用）".into(),
            ));
        }
        let handle = opened.expect("已确认可打开");
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        let moved = MoveFileExW(
            PCWSTR(wide(&tmp).as_ptr()),
            PCWSTR(wide(exe).as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        );
        if moved.is_err() {
            let _ = std::fs::remove_file(&tmp);
            return Err(KernelError::InvalidArgument(
                "替换游戏启动文件失败（游戏运行中时文件被独占）".into(),
            ));
        }
    }
    Ok(())
}

/// 备份原始启动文件（仅导入表注入时期使用；新的远程注入不再产生备份）。
pub fn ensure_backup(exe: &Path) -> Result<(), KernelError> {
    let backup = backup_path(exe);
    if backup.is_file() {
        return Ok(());
    }
    std::fs::copy(exe, &backup)?;
    Ok(())
}

/// 还原为未注入的原始启动文件（清理历史遗留注入用）。
#[cfg(windows)]
pub fn restore_original(exe: &Path) -> Result<(), KernelError> {
    let backup = backup_path(exe);
    if !backup.is_file() {
        return Err(KernelError::InvalidArgument(
            "没有可用的原始启动文件备份".into(),
        ));
    }
    let bytes = std::fs::read(&backup)?;
    replace_exe(exe, &bytes)
}

/// 非 Windows 平台没有注入，也就没有「还原」这一步。
#[cfg(not(windows))]
pub fn restore_original(_exe: &Path) -> Result<(), KernelError> {
    Err(KernelError::InvalidArgument(
        "当前平台不支持启动注入".into(),
    ))
}

/// 删除备份文件（释放磁盘）。
pub fn delete_backup(exe: &Path) -> Result<(), KernelError> {
    let backup = backup_path(exe);
    if backup.is_file() {
        std::fs::remove_file(&backup)?;
    }
    Ok(())
}

/// 执行启动准备：落 hook DLL + 清理历史遗留的导入表注入。
///
/// **不再改写 exe 的导入表。** 老的实现会在这一步给 exe 追加 `.copperh` 节，
/// 而那份镜像被 Windows 加载器拒绝（`0xC0000135`），表现为「点了开始游戏就
/// 悄悄没了」。现在检测到这种残留就自动从备份还原，让实例回到能启动的状态 ——
/// 用户不该为自己的历史操作买单，也不该为此手工删文件。
pub fn inject(version_dir: &Path, exe: &Path) -> Result<InjectOutcome, KernelError> {
    let dll_written = ensure_hook_dll(version_dir)?;
    let legacy_import_removed = if has_legacy_import(exe) {
        restore_original(exe)?;
        log::warn!(
            "[home/inject] 实例 `{}` 的启动文件仍带导入表注入，已从备份还原（改写 exe 的注入方式已废弃）",
            version_dir.display()
        );
        true
    } else {
        false
    };
    // 没有待还原的注入时，`.copperorig` 就是纯粹的死重量（一个完整 exe 副本，
    // 300MB 量级）。远程注入不再改写 exe，留着它没有任何用途。
    if !legacy_import_removed {
        if let Err(error) = delete_backup(exe) {
            log::warn!("[home/inject] 清理历史备份失败（可忽略）: {error}");
        }
    }
    Ok(InjectOutcome {
        dll_written,
        legacy_import_removed,
    })
}

/// 按版本设置应用/撤销控制台子系统。
///
/// 与注入共用同一个 PE 解析，但**只改一个字段**，不需要重排：改完立刻生效，
/// 撤销时把值改回即可。
#[cfg(windows)]
pub fn apply_console(exe: &Path, console: bool) -> Result<(), KernelError> {
    let bytes = std::fs::read(exe)?;
    let mut image = pe::parse(bytes).map_err(|e| pe_err("解析游戏启动文件失败", e))?;
    const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;
    const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;
    let want = if console {
        IMAGE_SUBSYSTEM_WINDOWS_CUI
    } else {
        IMAGE_SUBSYSTEM_WINDOWS_GUI
    };
    if image.subsystem() == want {
        return Ok(());
    }
    image.set_subsystem_in_place(want);
    let patched = image.into_data();
    replace_exe(exe, &patched)
}

#[cfg(not(windows))]
pub fn apply_console(_exe: &Path, _console: bool) -> Result<(), KernelError> {
    Ok(())
}

/// SHA-256（只为比对 DLL 是否已是最新，避免每次启动都重写文件）。
#[cfg(windows)]
fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_the_contract() {
        let dir = Path::new("D:/v/demo");
        assert!(hook_dll_path(dir).ends_with(contract::HOOK_DLL_FILE_NAME));
        assert!(manifest_path(dir).ends_with(contract::MANIFEST_FILE_NAME));
        let exe = Path::new("D:/v/demo/Minecraft.Windows.exe");
        assert!(backup_path(exe).to_string_lossy().ends_with(".copperorig"));
    }

    #[test]
    fn dll_presence_drives_injection_state() {
        let dir = std::env::temp_dir().join(format!("copper_inject_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!hook_dll_path(&dir).is_file());
        let exe = dir.join("Minecraft.Windows.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let state = launch_file_state(&dir, &exe, &VersionMeta::default());
        // DLL 未落位 → 注入不可用
        assert!(!state.injected);
        assert!(!state.backup_exists);

        // DLL 落位 + 启动文件没有导入注入 → 可用
        std::fs::write(hook_dll_path(&dir), b"x").unwrap();
        let state = launch_file_state(&dir, &exe, &VersionMeta::default());
        assert!(state.injected);
        assert!(state.hook_dll_present);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 没有待还原的历史注入时，`.copperorig` 必须被自动清掉（300MB 死重量）。
    #[test]
    fn stale_backup_is_reclaimed() {
        let dir = std::env::temp_dir().join(format!("copper_inject_bak_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Minecraft.Windows.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        std::fs::write(backup_path(&exe), b"backup").unwrap();
        assert!(launch_file_state(&dir, &exe, &VersionMeta::default()).backup_exists);

        // exe 不是合法 PE（非 Windows 的桩 / 损坏镜像），探测不到注入，
        // 因此走「清理备份」分支而不是「还原」分支。
        let outcome = inject(&dir, &exe).expect("注入准备不应失败");
        assert!(!outcome.legacy_import_removed);
        assert!(!backup_path(&exe).exists(), "死备份必须被回收");
        assert!(hook_dll_path(&dir).is_file(), "hook DLL 必须落位");
        let _ = std::fs::remove_dir_all(&dir);
    }
}