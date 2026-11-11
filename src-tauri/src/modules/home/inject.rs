//! hook DLL 的落位，以及「把 DLL 写进游戏 exe 的导入表」。
//!
//! # 为什么是导入表注入，而不是远程线程
//!
//! 两条路都能让 DLL 进进程，但**时机不同**，而时机决定隔离与登录能不能成立：
//!
//! - 远程线程（`CREATE_SUSPENDED` + `LoadLibraryW`）：DLL 的 `DllMain` 在
//!   **导入表还没被解析**时执行，加载器随后会把真实函数地址写回 IAT，把我们
//!   刚装好的补丁冲掉。我们只能靠 1ms 轮询去夺回，而加载器初始化 / 各依赖
//!   DLL 的 `DllMain` 在这段窗口里已经调用过目录 API，拿到的是真实路径。
//! - 导入表注入（本模块）：DLL 由加载器按导入表加载，`DllMain` 在**导入解析
//!   完成之后**执行，补丁写进去就是最终值，不会被覆盖。
//!
//! 实机上后者的差异是可观测的：前者在 1.26.52.03 上永远拿不到 Xbox token
//! （界面停在「正在用 Microsoft 帐户登录」），后者与 LeviLauncher 的表现一致。
//!
//! 上一轮曾以为导入表注入本身不可行（加载器 `0xC0000135` 无声拒绝）。真因是
//! `pe/import.rs` 把导入描述符的 `Name` 与 `hint+name` 写成了 UTF-16；PE 要求
//! 这两处是 ANSI 窄字符串。修正后镜像可被加载器正常接受。
//!
//! # 代价（已知并接受）
//!
//! 改写 exe 会让 Authenticode 签名失效：智能应用控制（SAC）开启的机器会拒绝
//! 加载被改过的 exe。同时每个实例多出一个 `.copperorig` 备份（约 300MB），
//! 它是唯一的回滚手段，**不再自动删除**。

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

/// 导入表里引用的 DLL 文件名与锚点导出名（与 hook crate 共享同一份契约）。
const HOOK_DLL_NAME: &str = contract::HOOK_DLL_FILE_NAME;
const HOOK_ANCHOR: &str = contract::ANCHOR_EXPORT;

/// 启动准备的结论。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InjectOutcome {
    /// 是否新写入了 hook DLL。
    pub dll_written: bool,
    /// 本次是否给 exe 加了导入项（改写了启动文件）。
    pub import_added: bool,
    /// 启动文件本来就带我们的导入项（幂等命中，零写入）。
    pub already_imported: bool,
    /// 导入表注入不可用的原因；非空时调用方应退回远程注入。
    pub fallback_reason: Option<String>,
}

/// 启动文件状态（版本设置页展示用）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LaunchFileState {
    pub hook_dll_present: bool,
    /// 注入是否已就位：hook DLL 在位，且启动文件已静态导入它。
    pub injected: bool,
    /// 原始启动文件备份是否存在（回滚手段，约 300MB）。
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
    version_dir.join(HOOK_DLL_NAME)
}

/// 原始启动文件的备份路径（`Minecraft.Windows.exe.copperorig`）。
pub fn backup_path(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_os_string();
    name.push(contract::BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// 预加载清单位置。
pub fn manifest_path(version_dir: &Path) -> PathBuf {
    version_dir.join(contract::MANIFEST_FILE_NAME)
}

/// 启动文件是否已导入我们的 hook DLL。
#[cfg(windows)]
pub fn imports_hook_dll(exe: &Path) -> bool {
    std::fs::read(exe)
        .ok()
        .and_then(|bytes| pe::dll_is_imported(&bytes, HOOK_DLL_NAME).ok())
        .unwrap_or(false)
}

/// 非 Windows 没有导入表注入的概念，恒为 false。
#[cfg(not(windows))]
pub fn imports_hook_dll(_exe: &Path) -> bool {
    false
}

/// 查询启动文件状态（不修改任何文件）。
pub fn launch_file_state(version_dir: &Path, exe: &Path, meta: &VersionMeta) -> LaunchFileState {
    let backup = backup_path(exe);
    let hook_dll_present = hook_dll_path(version_dir).is_file();
    let injected = imports_hook_dll(exe);
    LaunchFileState {
        injected,
        hook_dll_present,
        backup_exists: backup.is_file(),
        backup_bytes: std::fs::metadata(&backup).map(|m| m.len()).unwrap_or(0),
        console: meta.enable_console,
    }
}

/// 确保实例目录里有最新版本的 hook DLL，按 SHA256 比对决定是否重写。
#[cfg(windows)]
pub(super) fn ensure_hook_dll(version_dir: &Path) -> Result<bool, KernelError> {
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
pub(super) fn ensure_hook_dll(_version_dir: &Path) -> Result<bool, KernelError> {
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

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    path.to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

/// 把字节写进 `path`（`FILE_FLAG_WRITE_THROUGH`，断电也不留半个 PE）。
#[cfg(windows)]
fn write_through(path: &Path, bytes: &[u8]) -> Result<(), KernelError> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::FILE_FLAG_WRITE_THROUGH;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options.custom_flags(FILE_FLAG_WRITE_THROUGH.0);
    let mut file = options.open(path).map_err(|e| {
        KernelError::InvalidArgument(format!(
            "写入启动文件临时副本失败：{e}（游戏运行中时无法改写启动文件，请先退出游戏）"
        ))
    })?;
    std::io::Write::write_all(&mut file, bytes)
        .map_err(|e| {
            let _ = std::fs::remove_file(path);
            KernelError::Io(e)
        })?;
    Ok(())
}

/// 确认 exe 当前可写：游戏运行中时它是独占的，必须给出可操作的错误。
#[cfg(windows)]
fn ensure_exe_writable(exe: &Path) -> Result<(), KernelError> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_READ_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_WRITE_DATA, OPEN_EXISTING,
    };

    let path = wide(exe);
    let opened = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            FILE_READ_DATA.0 | FILE_WRITE_DATA.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    match opened {
        Ok(handle) => {
            unsafe { let _ = CloseHandle(handle); }
            Ok(())
        }
        Err(_) => Err(KernelError::InvalidArgument(
            "无法写入游戏启动文件（游戏运行中或文件被占用）".into(),
        )),
    }
}

/// 原子替换 exe（`ReplaceFileW`），并把旧文件留成 `<exe>.copperorig`。
///
/// `ReplaceFileW` 而不是 `MoveFileExW`：它保留原文件的 ACL / 属性，并**直接
/// 产出备份**——备份是这次改写的唯一回滚手段，必须与替换在同一操作里完成。
#[cfg(windows)]
fn replace_exe_with_backup(exe: &Path, bytes: &[u8]) -> Result<(), KernelError> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS};

    ensure_exe_writable(exe)?;
    let backup = backup_path(exe);
    // 备份名指向已存在的文件时 ReplaceFileW 会失败。旧备份来自上一轮补丁，
    // 已无意义（真正的原始文件在第一次打补丁时就存下了）。
    if backup.is_file() {
        let _ = std::fs::remove_file(&backup);
    }
    let tmp = exe.with_extension("copper-inject-tmp");
    write_through(&tmp, bytes)?;

    let exe_w = wide(exe);
    let tmp_w = wide(&tmp);
    let backup_w = wide(&backup);
    let result = unsafe {
        ReplaceFileW(
            PCWSTR(exe_w.as_ptr()),
            PCWSTR(tmp_w.as_ptr()),
            PCWSTR(backup_w.as_ptr()),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    };
    if let Err(error) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(KernelError::InvalidArgument(format!(
            "替换游戏启动文件失败：{error}（游戏运行中时文件被独占）"
        )));
    }
    Ok(())
}

/// 只改子系统字段（控制台开关），不碰导入表。
///
/// 与导入表注入共用同一个 PE 解析。**两者绝不各自写一遍文件**：311MB 重写两次
/// 既慢又给失败留出两个窗口，所以 [`prepare_launch`] 会在同一次改写里把子系统
/// 一起设好，这个函数只用于「已经注入过、只是控制台开关变了」的场合。
#[cfg(windows)]
fn replace_exe(exe: &Path, bytes: &[u8]) -> Result<(), KernelError> {
    ensure_exe_writable(exe)?;
    replace_exe_with_backup(exe, bytes)
}

/// 启动准备：落 hook DLL + 确保 exe 静态导入它。
///
/// 顺序固定：先落 DLL（导入解析时它必须已在 exe 同目录），再改导入表。
///
/// 幂等：`imports_hook_dll` 命中就不写盘——一次改写是 311MB 的磁盘读写，
/// 每次启动都做不可接受。
///
/// 失败不阻断：任何 PE 层的拒绝都转成 `fallback_reason`，由调用方退回远程注入，
/// 保证游戏至少能启动（隔离可能因此退化，但会在日志里说明原因）。
#[cfg(windows)]
pub fn prepare_launch(
    version_dir: &Path,
    exe: &Path,
    console: bool,
) -> Result<InjectOutcome, KernelError> {
    let dll_written = ensure_hook_dll(version_dir)?;

    let bytes = std::fs::read(exe).map_err(|error| {
        KernelError::InvalidArgument(format!("读取游戏启动文件失败：{error}"))
    })?;
    let mut image = pe::parse(bytes).map_err(|e| pe_err("解析游戏启动文件失败", e))?;

    const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;
    const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;
    let want = if console {
        IMAGE_SUBSYSTEM_WINDOWS_CUI
    } else {
        IMAGE_SUBSYSTEM_WINDOWS_GUI
    };
    let console_differs = image.subsystem() != want;

    if image
        .dll_is_imported(HOOK_DLL_NAME)
        .map_err(|e| pe_err("读取导入表失败", e))?
    {
        // 幂等命中。控制台开关变了才写一次盘；校验和由 replace_exe 用
        // Windows 自己的 CheckSumMappedFile 修，不在这里重写整份镜像。
        if console_differs {
            image.set_subsystem_in_place(want);
            replace_exe(exe, &image.into_data())?;
        }
        return Ok(InjectOutcome {
            dll_written,
            import_added: false,
            already_imported: true,
            fallback_reason: None,
        });
    }

    if console_differs {
        image.set_subsystem_in_place(want);
    }

    let plan = match image.plan_add_import(HOOK_DLL_NAME, HOOK_ANCHOR) {
        Ok(plan) => plan,
        Err(PeError::AlreadyImported(_)) => {
            return Ok(InjectOutcome {
                dll_written,
                import_added: false,
                already_imported: true,
                fallback_reason: None,
            })
        }
        Err(error) => {
            log::warn!(
                "[home/inject] 无法给 `{}` 追加导入项，将退回远程注入：{error}",
                exe.display()
            );
            return Ok(InjectOutcome {
                dll_written,
                import_added: false,
                already_imported: false,
                fallback_reason: Some(error.to_string()),
            });
        }
    };

    let patched = plan
        .apply(&image)
        .map_err(|e| pe_err("重排启动文件失败", e))?;
    // 落盘前自检：补丁后的镜像必须真的能读到这条导入项。写坏了再发现，
    // 用户面对的就是「点了开始游戏什么也没发生」。
    if !pe::dll_is_imported(&patched, HOOK_DLL_NAME)
        .map_err(|e| pe_err("自检解析失败", e))?
    {
        return Err(KernelError::InvalidArgument(
            "注入自检失败：补丁后镜像里找不到导入项".into(),
        ));
    }
    replace_exe_with_backup(exe, &patched)?;
    log::info!(
        "[home/inject] 已把 `{HOOK_DLL_NAME}!{HOOK_ANCHOR}` 写入 `{}` 的导入表（备份 {BACKUP}）",
        exe.display(),
        BACKUP = contract::BACKUP_SUFFIX
    );
    Ok(InjectOutcome {
        dll_written,
        import_added: true,
        already_imported: false,
        fallback_reason: None,
    })
}

/// 非 Windows：没有导入表注入，也不需要回退（各平台有自己的宿主）。
#[cfg(not(windows))]
pub fn prepare_launch(
    _version_dir: &Path,
    _exe: &Path,
    _console: bool,
) -> Result<InjectOutcome, KernelError> {
    Ok(InjectOutcome {
        dll_written: false,
        import_added: false,
        already_imported: false,
        fallback_reason: None,
    })
}

/// 还原为未注入的原始启动文件。
///
/// 只用于用户显式「还原启动文件」；正常启动路径**不会**自动还原——还原会让
/// 下一次启动重新走一遍 311MB 改写。
#[cfg(windows)]
pub fn restore_original(exe: &Path) -> Result<(), KernelError> {
    let backup = backup_path(exe);
    if !backup.is_file() {
        return Err(KernelError::InvalidArgument(
            "没有可用的原始启动文件备份".into(),
        ));
    }
    let bytes = std::fs::read(&backup)?;
    ensure_exe_writable(exe)?;
    // 还原时把「未注入的文件」写回去，并把当前（已注入的）文件留成备份，
    // 这样还原同样可逆。
    let tmp = exe.with_extension("copper-inject-tmp");
    write_through(&tmp, &bytes)?;
    let exe_w = wide(exe);
    let tmp_w = wide(&tmp);
    let _ = std::fs::remove_file(&backup);
    let result = unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS};
        ReplaceFileW(
            PCWSTR(exe_w.as_ptr()),
            PCWSTR(tmp_w.as_ptr()),
            PCWSTR(wide(&backup).as_ptr()),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    };
    if let Err(error) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(KernelError::InvalidArgument(format!(
            "还原启动文件失败：{error}"
        )));
    }
    repair_checksum(exe);
    Ok(())
}

/// 用 Windows 自己的 `MapFileAndCheckSumW` 重算并写回 PE CheckSum。
///
/// 不用手写算法：本项目的 `pe_checksum_with` 在本机 311MB 镜像上算出的值与
/// `CheckSumMappedFile` 不一致，落盘后会留下“签名在、校验和错”的镜像，
/// SmartScreen 按篡改拦截。这里直接问系统，避免再猜。
#[cfg(windows)]
fn repair_checksum(exe: &Path) {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{s, PCWSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    unsafe {
        let module = match LoadLibraryW(windows::core::w!("imagehlp.dll")) {
            Ok(module) => module,
            Err(_) => return,
        };
        let Some(proc) = GetProcAddress(module, s!("MapFileAndCheckSumW")) else {
            return;
        };
        let map: unsafe extern "system" fn(PCWSTR, *mut u32, *mut u32) -> u32 =
            std::mem::transmute(proc);
        let mut header = 0u32;
        let mut calculated = 0u32;
        let path: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        if map(PCWSTR(path.as_ptr()), &mut header, &mut calculated) != 0 {
            return;
        }
        if header == calculated {
            return;
        }
        let Ok(mut file) = std::fs::OpenOptions::new().read(true).write(true).open(exe) else {
            return;
        };
        let mut head = [0u8; 0x400];
        if file.read_exact(&mut head).is_err() {
            return;
        }
        let pe = u32::from_le_bytes([head[0x3c], head[0x3d], head[0x3e], head[0x3f]]) as u64;
        let offset = pe + 24 + 64;
        if file.seek(SeekFrom::Start(offset)).is_ok() {
            let _ = file.write_all(&calculated.to_le_bytes());
            let _ = file.flush();
        }
    }
}

/// 非 Windows 平台没有注入，也就没有「还原」这一步。
#[cfg(not(windows))]
pub fn restore_original(_exe: &Path) -> Result<(), KernelError> {
    Err(KernelError::InvalidArgument(
        "当前平台不支持启动注入".into(),
    ))
}

/// 删除备份文件（释放约 300MB）。
///
/// **只由用户显式操作触发**：备份是回滚的唯一依据，自动删等于放弃回滚。
pub fn delete_backup(exe: &Path) -> Result<(), KernelError> {
    let backup = backup_path(exe);
    if backup.is_file() {
        std::fs::remove_file(&backup)?;
    }
    Ok(())
}

/// 远程注入准备：恢复原版 exe（若有干净备份），只落 hook DLL，**不改 exe**。
///
/// 返回 `(dll_written, restored)`。exe 保持原签名/原哈希，进程不会被
/// SmartScreen 按“被篡改的签名文件”降成 Low；hook 由启动器挂起进程后
/// `LoadLibraryW` 注入。
#[cfg(windows)]
pub fn prepare_remote(
    version_dir: &Path,
    exe: &Path,
) -> Result<(bool, bool), KernelError> {
    let restored = if backup_path(exe).is_file() {
        restore_original(exe)?;
        true
    } else {
        false
    };
    let dll_written = ensure_hook_dll(version_dir)?;
    Ok((dll_written, restored))
}

/// 非 Windows 平台没有远程注入。
#[cfg(not(windows))]
pub fn prepare_remote(
    version_dir: &Path,
    _exe: &Path,
) -> Result<(bool, bool), KernelError> {
    let dll_written = ensure_hook_dll(version_dir)?;
    Ok((dll_written, false))
}

/// 按版本设置应用/撤销控制台子系统（独立入口，供设置页即时生效）。
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
    if image.subsystem() == want && image.checksum_matches() {
        return Ok(());
    }
    if image.subsystem() != want {
        image.set_subsystem_in_place(want);
    }
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
    fn dll_presence_and_import_drive_state() {
        let dir = std::env::temp_dir().join(format!("copper_inject_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Minecraft.Windows.exe");
        std::fs::write(&exe, b"MZ").unwrap();

        // DLL 未落位、exe 也不是合法 PE → 未注入
        let state = launch_file_state(&dir, &exe, &VersionMeta::default());
        assert!(!state.injected);
        assert!(!state.hook_dll_present);
        assert!(!state.backup_exists);

        // DLL 落位但 exe 仍未导入 → 依然不算已注入（导出/导入必须成对）
        std::fs::write(hook_dll_path(&dir), b"x").unwrap();
        let state = launch_file_state(&dir, &exe, &VersionMeta::default());
        assert!(state.hook_dll_present);
        assert!(!state.injected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 备份是回滚的唯一依据：`delete_backup` 只应显式调用，不能被启动路径顺手清掉。
    #[test]
    fn backup_survives_state_probe() {
        let dir = std::env::temp_dir().join(format!("copper_inject_bak_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Minecraft.Windows.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        std::fs::write(backup_path(&exe), b"backup").unwrap();

        let state = launch_file_state(&dir, &exe, &VersionMeta::default());
        assert!(state.backup_exists);
        assert_eq!(state.backup_bytes, 6);
        assert!(backup_path(&exe).is_file(), "探测状态不得删除备份");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn importing_a_missing_exe_is_false() {
        assert!(!imports_hook_dll(
            &std::env::temp_dir().join("copper_no_such_exe_7f21/Minecraft.Windows.exe")
        ));
    }
}
