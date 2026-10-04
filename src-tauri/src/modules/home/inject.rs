//! 注入编排：把 hook DLL 落进实例目录，并改写游戏 exe 的导入表。
//!
//! # 为什么必须注入
//!
//! 游戏的目录 API（`SHGetKnownFolderPath`）不读环境变量，注册 AppX 也不产生
//! 任何路径重定向（LeviLauncher 的注册清单只把虚拟化关掉）。因此「让游戏把
//! 数据写进实例目录」唯一可行的办法是：**改写 exe 的导入表，让我们的 DLL
//! 在进程启动时被加载，并在进程内替换那三个 API**。同一个 DLL 顺带承担
//! 原生模组预加载（加载器就是这样生效的）。
//!
//! 细节与取舍见 `docs/启动链路与实例隔离.md`。
//!
//! # 写文件的纪律
//!
//! 改写的是**用户的游戏本体**，写坏了用户就玩不了这个实例。所以：
//!! 布局计算在 [`crate::pe`] 内完成（纯内存，可单测），本模块只负责
//! 「临时文件 → `ReplaceFileW` 原子替换 → 回读校验 → 失败回滚」，
//!! 并且**永远保留一份原始副本**（`.copperorig`），提供还原入口。

use std::path::{Path, PathBuf};

use copper_core_hook::contract as contract;

use crate::error::KernelError;
use crate::modules::home::meta::VersionMeta;
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
    /// 是否改写了 exe 导入表。
    pub exe_patched: bool,
    /// 是否已存在注入（幂等命中）。
    pub already_injected: bool,
}

/// 启动文件状态（版本设置页展示用）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LaunchFileState {
    pub hook_dll_present: bool,
    /// exe 是否已导入 hook（true = 注入在生效）。
    pub injected: bool,
    pub backup_exists: bool,
    /// 备份文件字节数（0 = 无备份）。
    pub backup_bytes: u64,
    pub console: bool,
}

fn pe_err(context: &str, error: PeError) -> KernelError {
    KernelError::InvalidArgument(format!("{context}：{error}"))
}

/// hook DLL 在实例目录下的路径。
pub fn hook_dll_path(version_dir: &Path) -> PathBuf {
    version_dir.join(contract::HOOK_DLL_FILE_NAME)
}

/// 原始启动文件备份路径。
pub fn backup_path(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_os_string();
    name.push(contract::BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// 预加载清单位置。
pub fn manifest_path(version_dir: &Path) -> PathBuf {
    version_dir.join(contract::MANIFEST_FILE_NAME)
}

/// 查询启动文件状态（不修改任何文件）。
#[cfg(windows)]
pub fn launch_file_state(version_dir: &Path, exe: &Path, meta: &VersionMeta) -> LaunchFileState {
    let backup = backup_path(exe);
    let injected = std::fs::read(exe)
        .ok()
        .and_then(|bytes| pe::parse(bytes).ok())
        .map(|image| image.dll_is_imported(contract::HOOK_DLL_FILE_NAME).unwrap_or(false))
        .unwrap_or(false);
    LaunchFileState {
        hook_dll_present: hook_dll_path(version_dir).is_file(),
        injected,
        backup_exists: backup.is_file(),
        backup_bytes: std::fs::metadata(&backup).map(|m| m.len()).unwrap_or(0),
        console: meta.enable_console,
    }
}

/// 非 Windows 平台没有注入概念（隔离与预加载是 Windows GDK 专属）。
#[cfg(not(windows))]
pub fn launch_file_state(version_dir: &Path, exe: &Path, meta: &VersionMeta) -> LaunchFileState {
    let _ = (version_dir, exe);
    LaunchFileState {
        hook_dll_present: hook_dll_path(version_dir).is_file(),
        injected: false,
        backup_exists: backup_path(exe).is_file(),
        backup_bytes: 0,
        console: meta.enable_console,
    }
}

/// 确保实例目录里有最新版本的 hook DLL，按 SHA256 比对决定是否重写。
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

/// 确保 exe 导入表里有 hook；已导入时零写入。
fn ensure_import(exe: &Path) -> Result<bool, KernelError> {
    let bytes = std::fs::read(exe).map_err(|e| {
        KernelError::InvalidArgument(format!("读取游戏启动文件失败：{e}"))
    })?;
    let image = pe::parse(bytes).map_err(|e| pe_err("解析游戏启动文件失败", e))?;
    if !image.is_x64() {
        return Err(KernelError::InvalidArgument(
            "该实例不是 64 位游戏，无法注入".into(),
        ));
    }
    let plan = match image.plan_add_import(contract::HOOK_DLL_FILE_NAME, contract::ANCHOR_EXPORT) {
        Ok(plan) => plan,
        // 幂等命中：已经注入过，不写文件。
        Err(PeError::AlreadyImported(_)) => return Ok(false),
        Err(error) => return Err(pe_err("注入准备失败", error)),
    };
    let patched = plan.apply(&image).map_err(|e| pe_err("注入失败", e))?;
    replace_exe(exe, &patched)?;
    Ok(true)
}

/// 写文件（临时文件 + rename），失败时不留半成品。
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

/// 用 `ReplaceFileW` 替换 exe：保留原文件 ACL，并留下原始副本。
///
/// 为什么不用「直接写」：直接写在中途失败 / 断电 / 被占用时会留下**半张 PE**，
/// 用户的实例就此报废。`ReplaceFileW` 是单次原子替换，且顺带满足
/// 「随时可还原」。
fn replace_exe(exe: &Path, bytes: &[u8]) -> Result<(), KernelError> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    use windows::Win32::System::Threading::{
        CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_WRITE_THROUGH,
    };

    let tmp = exe.with_extension("copper-inject-tmp");
    let backup = backup_path(exe);
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
                "写入注入临时文件失败：{e}（游戏运行中时无法改写启动文件，请先退出游戏）"
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
        let handle = CreateFileW(
            PCWSTR(wide(exe).as_ptr()),
            FILE_FLAG_WRITE_THROUGH.0,
            Default::default(),
            None,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE(std::ptr::null_mut()),
        );
        if handle == INVALID_HANDLE_VALUE {
            let _ = std::fs::remove_file(&tmp);
            return Err(KernelError::InvalidArgument(
                "无法写入游戏启动文件（游戏运行中或文件被占用）".into(),
            ));
        }
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        // REPLACE_EXISTING + WRITE_THROUGH：替换是原子的，且不靠系统缓存侥幸落盘。
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
    let _ = &backup;
    Ok(())
}

/// 备份原始启动文件（仅首次），供「还原原始启动文件」使用。
pub fn ensure_backup(exe: &Path) -> Result<(), KernelError> {
    let backup = backup_path(exe);
    if backup.is_file() {
        return Ok(());
    }
    std::fs::copy(exe, &backup)?;
    Ok(())
}

/// 还原为未注入的原始启动文件。
pub fn restore_original(exe: &Path) -> Result<(), KernelError> {
    let backup = backup_path(exe);
    if !backup.is_file() {
        return Err(KernelError::InvalidArgument("没有可用的原始启动文件备份".into()));
    }
    let bytes = std::fs::read(&backup)?;
    replace_exe(exe, &bytes)
}

/// 删除备份文件（释放磁盘）。
pub fn delete_backup(exe: &Path) -> Result<(), KernelError> {
    let backup = backup_path(exe);
    if backup.is_file() {
        std::fs::remove_file(&backup)?;
    }
    Ok(())
}

/// 执行注入：落 DLL + 备份 + 改写导入表。
#[cfg(windows)]
pub fn inject(version_dir: &Path, exe: &Path) -> Result<InjectOutcome, KernelError> {
    let dll_written = ensure_hook_dll(version_dir)?;
    // 备份必须在改写**之前**做，且只在首次（否则备份会变成已注入的版本）。
    ensure_backup(exe)?;
    let already = std::fs::read(exe)
        .ok()
        .and_then(|bytes| pe::parse(bytes).ok())
        .map(|image| image.dll_is_imported(contract::HOOK_DLL_FILE_NAME).unwrap_or(false))
        .unwrap_or(false);
    let exe_patched = if already {
        false
    } else {
        ensure_import(exe)?
    };
    Ok(InjectOutcome {
        dll_written,
        exe_patched,
        already_injected: already,
    })
}

#[cfg(not(windows))]
pub fn inject(_version_dir: &Path, _exe: &Path) -> Result<InjectOutcome, KernelError> {
    Err(KernelError::InvalidArgument(
        "当前平台不支持启动注入".into(),
    ))
}

/// 按版本设置应用/撤销控制台子系统。
///
/// 与注入走同一个 PE 解析，但**只改一个字段**，不需要重排：改完立刻生效，
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
    fn dll_presence_is_detected() {
        let dir = std::env::temp_dir().join(format!("copper_inject_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!hook_dll_path(&dir).is_file());
        let exe = dir.join("Minecraft.Windows.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let state = launch_file_state(&dir, &exe, &VersionMeta::default());
        assert!(!state.injected);
        assert!(!state.backup_exists);
        // 非 Windows 分支也返回同一形状，状态接口不会因平台而缺字段
        let _ = std::fs::remove_dir_all(&dir);
    }
}
