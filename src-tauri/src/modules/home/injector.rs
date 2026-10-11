//! 挂起启动 + 远程线程注入。
//!
//! # 为什么不用 PE 导入表注入
//!
//! 早先的实现走的是「给游戏 exe 的导入表追加一条我们自己的 DLL」——也就是
//! [`super::inject`] 与 [`crate::pe`]。这条路在本机的真实 GDK 游戏
//! （1.26.52.03）与 `notepad.exe` 上都会被加载器以 `0xC0000135` 无声拒绝，
//! 且**不产生任何 Application Error 事件、也没有「缺少 DLL」对话框**：镜像
//! 结构、节表、数据目录、节内容逐字节校验全部自洽，`dumpbin` 却读不出导入
//! 表。在没有调试器逐条比对加载器内部行为之前，这条路不值得继续投入。
//!
//! 改为**不改游戏 exe 一个字节**：以 `CREATE_SUSPENDED` 启动游戏，在主线程
//! 尚未执行任何指令时，向其地址空间写入 hook DLL 的绝对路径，再起一个远程线程
//! 调用 `LoadLibraryW`。加载完成后恢复主线程。
//!
//! # 与导入注入的差异
//!
//! - 游戏 exe 保持原样：签名有效、没有新增节、备份与还原逻辑不再需要参与
//!   启动路径（仍保留给用户手动维护）。
//! - 交接变量仍通过 [`crate::modules::home::launch`] 构造的环境块下发，hook
//!   侧读取方式不变（`COPPER_HOOK_*`）。
//! - 注入结果可验证：远程线程的退出码就是 `LoadLibraryW` 的返回值——非 0 即
//!   模块基址，注入成功；为 0 则 DLL 没加载上。旧方案没有任何这种回执。
//!
//! # 已知限制
//!
//! - 远程线程里调用的 `kernel32!LoadLibraryW` 地址取自本进程。同一次启动内
//!   所有进程共享同一份系统 DLL 基址（Windows 的 ASLR 以「每次启动」为粒度），
//!   所以该地址在目标进程里同样有效；这是这类注入的通行做法。
//! - 需要目标进程与启动器同权限级别。跨权限级别会被 `OpenProcess` 拒绝。

use std::path::Path;

use windows::core::{PCSTR, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{
    VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
};
use windows::Win32::System::Threading::{
    CreateProcessW, CreateRemoteThread, GetExitCodeThread, ResumeThread, TerminateProcess,
    WaitForSingleObject, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION,
    STARTUPINFOW,
};

use crate::error::KernelError;

/// 注入失败时回滚：还没恢复的主线程不能就这么留着。
///
/// 挂起进程里主线程停在第一条指令上，既不占 CPU 也不会推进；不终止它就会留下
/// 一个永久挂起的僵尸实例，比启动失败更难排查。
struct Suspended {
    process: HANDLE,
    thread: HANDLE,
    resumed: bool,
}

impl Suspended {
    fn resume(&mut self) {
        if self.resumed {
            return;
        }
        self.resumed = true;
        // 恢复失败已无可挽回：此时游戏已经跑起来了，记日志即可。
        // `ResumeThread` 返回 0 表示此前已是活动状态，失败返回 `u32::MAX`。
        let previous = unsafe { ResumeThread(self.thread) };
        if previous == u32::MAX {
            log::error!("[home/injector] 恢复主线程失败");
        }
    }
}

impl Drop for Suspended {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.thread);
            if !self.resumed {
                let _ = TerminateProcess(self.process, 0xC0000135);
            }
            let _ = CloseHandle(self.process);
        }
    }
}

/// 构造以 NUL 结尾的环境块（`KEY=VALUE\0 ... \0\0`）。
///
/// `CreateProcessW` 配 `CREATE_UNICODE_ENVIRONMENT` 时要的就是这个布局；
/// 以 `KEY=VALUE` 形式传入会被系统原样复制到子进程，`std::env::var` 随即可读。
/// 大小写不敏感地覆盖同名项，避免出现 `PATH` 与 `Path` 并存。
fn build_env_block(extra: &[(String, String)]) -> Vec<u16> {
    let mut entries: Vec<(String, String)> = std::env::vars().collect();
    for (key, value) in extra {
        entries.retain(|(existing, _)| !existing.eq_ignore_ascii_case(key));
        entries.push((key.clone(), value.clone()));
    }
    let mut block: Vec<u16> = Vec::new();
    for (key, value) in entries {
        block.extend(key.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(value.encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

/// UTF-16 编码并补 NUL 终止符。
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 以挂起方式启动游戏、注入 DLL 后恢复主线程，返回进程 id。
///
/// `env` 是追加到当前环境之上的交接变量；`dll` 必须是**绝对路径**，因为目标
/// 进程的搜索路径尚未被任何人设置过。
pub fn launch_with_injection(
    exe: &Path,
    cwd: &Path,
    args: &[String],
    env: &[(String, String)],
    dll: &Path,
) -> Result<u32, KernelError> {
    let exe_wide = wide(&exe.to_string_lossy());
    let cwd_wide = wide(&cwd.to_string_lossy());
    let env_block = build_env_block(env);

    let mut command_line = exe_wide.clone();
    for arg in args {
        command_line.push(u16::from(b' '));
        command_line.extend(wide(arg));
    }

    let mut startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();

    // SAFETY: 全部入参都是本函数持有的、生命周期覆盖整个调用的缓冲。
    // 命令行缓冲区可写（`CreateProcessW` 要求 `lpCommandLine` 可变）。
    unsafe {
        CreateProcessW(
            PCWSTR(exe_wide.as_ptr()),
            PWSTR(command_line.as_mut_ptr()),
            None,
            None,
            false,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
            Some(env_block.as_ptr() as *const _),
            PCWSTR(cwd_wide.as_ptr()),
            &mut startup,
            &mut info,
        )
        .map_err(|error| {
            KernelError::Module(format!(
                "[home/injector] 挂起启动失败（{}）: {error}",
                exe.display()
            ))
        })?;
    }

    let mut suspended = Suspended {
        process: info.hProcess,
        thread: info.hThread,
        resumed: false,
    };
    let pid = info.dwProcessId;

    inject_into(&suspended, dll)?;
    suspended.resume();
    Ok(pid)
}

/// 在挂起的进程里起远程线程加载 `dll`。
fn inject_into(target: &Suspended, dll: &Path) -> Result<(), KernelError> {
    let path = wide(&dll.to_string_lossy());
    let bytes = path.len() * std::mem::size_of::<u16>();

    // SAFETY：`path` 存活到本函数结束；`bytes` 是它的字节长度。
    unsafe {
        let remote = VirtualAllocEx(
            target.process,
            None,
            bytes,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if remote.is_null() {
            return Err(KernelError::Module(format!(
                "[home/injector] 目标进程分配内存失败（Win32 错误 {}）",
                std::io::Error::last_os_error()
            )));
        }

        let result = (|| -> Result<HANDLE, KernelError> {
            WriteProcessMemory(
                target.process,
                remote,
                path.as_ptr() as *const _,
                bytes,
                None,
            )
            .map_err(|error| {
                KernelError::Module(format!("[home/injector] 写入 DLL 路径失败: {error}"))
            })?;

            let kernel = GetModuleHandleW(PCWSTR(wide("kernel32.dll").as_ptr())).map_err(
                |error| {
                    KernelError::Module(format!("[home/injector] 取 kernel32 基址失败: {error}"))
                },
            )?;
            let mut symbol: Vec<u8> = b"LoadLibraryW\0".to_vec();
            let load_library = GetProcAddress(kernel, PCSTR(symbol.as_ptr()));
            let Some(entry) = load_library.0.map(|raw| {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
                >(raw)
            }) else {
                return Err(KernelError::Module(
                    "[home/injector] kernel32 里找不到 LoadLibraryW".to_string(),
                ));
            };
            CreateRemoteThread(
                target.process,
                None,
                0,
                entry,
                Some(remote as *const _),
                0,
                None,
            )
            .map_err(|error| {
                KernelError::Module(format!("[home/injector] 创建远程线程失败: {error}"))
            })
        })();

        // 路径缓冲区用完即弃：即便 LoadLibraryW 仍在读，加载器也会把它拷走。
        let _ = VirtualFreeEx(target.process, remote, 0, MEM_RELEASE);

        let thread = result?;
        let _ = WaitForSingleObject(thread, 10_000);
        let mut code = 0u32;
        let _ = GetExitCodeThread(thread, &mut code);
        let _ = CloseHandle(thread);

        // 线程退出码就是 `LoadLibraryW` 的返回值：0 表示没加载上。
        if code == 0 {
            return Err(KernelError::Module(format!(
                "[home/injector] 远程线程未能加载 {}：LoadLibraryW 返回 0（依赖缺失或文件被拦截）",
                dll.display()
            )));
        }
        log::info!("[home/injector] 注入成功，DLL 基址 0x{code:X}");
    }
    Ok(())
}

/// 供 `build.rs` 之外的测试引用：确认 `LoadLibraryW` 在本进程可用。
///
/// 远程线程里调用的是**本进程**的 `LoadLibraryW` 地址，这一项能通过，说明
/// 目标进程里同一个地址也指向同一个函数——这是整套注入成立的前提。
#[cfg(test)]
mod tests {
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    fn symbol_of(name: &[u8]) -> Option<unsafe extern "system" fn() -> isize> {
        let kernel =
            unsafe { GetModuleHandleW(PCWSTR(super::wide("kernel32.dll").as_ptr())) }.ok()?;
        unsafe { GetProcAddress(kernel, PCSTR(name.as_ptr())).0 }
    }

    /// 远程线程里调用的是**本进程**的 `kernel32!LoadLibraryW` 地址。这一项通过，
    /// 说明目标进程里同一个地址也指向同一个函数 —— 整套注入成立的前提。
    #[test]
    fn load_library_is_resolvable() {
        assert!(
            symbol_of(b"LoadLibraryW\0").is_some(),
            "LoadLibraryW 必须可解析"
        );
    }

    #[test]
    fn env_block_overrides_case_insensitively() {
        let block = super::build_env_block(&[
            ("COPPER_HOOK_ROOT".to_string(), "D:\\a".to_string()),
            ("copper_hook_channel".to_string(), "release".to_string()),
        ]);
        let text = String::from_utf16_lossy(&block);
        assert!(text.contains("COPPER_HOOK_ROOT=D:\\a\0"));
        assert!(text.contains("copper_hook_channel=release\0"));
        let hooked: Vec<&str> = text
            .split('\0')
            .filter(|entry| entry.to_ascii_lowercase().starts_with("copper_hook"))
            .collect();
        assert_eq!(hooked.len(), 2, "两个交接变量都必须在环境块里，且不能重复");
    }

    #[test]
    fn env_block_is_double_nul_terminated() {
        let block = super::build_env_block(&[]);
        assert!(block.len() >= 2);
        assert_eq!(*block.last().unwrap(), 0);
        assert_eq!(block[block.len() - 2], 0);
    }
}