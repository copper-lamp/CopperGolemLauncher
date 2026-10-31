//! UWP 包的系统侧操作：查询 / 松散注册 / 卸载 / 激活 / 数据目录重定向。
//!
//! # 为什么是「松散注册」而不是「装进 WindowsApps」
//!
//! 我们的实例目录本身就是解包产物。`Add-AppxPackage -Register`（WinRT 侧是
//! `PackageManager::RegisterPackageByUriAsync`）把这个目录登记成**开发模式的松散包**：
//! 包身份、激活、`ApplicationData` 都按商店包那样工作，但文件仍留在版本目录里。
//! 这是「解包 → 可启动」之间缺的那一环，也是本模块存在的唯一原因。
//!
//! # 实测钉死的三条硬约束
//!
//! 1. **不能保留包签名**：包内有 `AppxSignature.p7x` 时注册会被拒（`0x80073CFF`
//!    「Windows Store 源的程序包不能使用开发人员模式选项端加载」）。解包器因此跳过
//!    签名文件（见 `uwp.rs`），这里不做任何补救——复制/删除签名都不该由部署层负责。
//! 2. **不能改全信任清单**：给 `Application` 加 `uap10:TrustLevel="mediumIL"` 并加
//!    `rescap:runFullTrust` 之后（LeviLauncher 的做法，用于让桌面 DLL 进入进程），
//!    MCBE `1.21.114.1` **启动即崩**（`0xe06d7363`，KERNELBASE RaiseException）；
//!    纯 AppContainer 注册则正常进入游戏。因此本模块注册时**原样使用包内清单**，
//!    一个字节都不改。代价是 UWP 实例不支持 LeviLamina（与 LeviLauncher 对 UWP
//!    拒绝加载器一致），这条限制由上层在安装期拦住。
//! 3. **不能替换包状态目录本身**：`%LOCALAPPDATA%\Packages\<族名>\LocalState` 由系统
//!    托管（安装卷非系统盘时它是指向 `<盘>\WpSystem\<SID>\...` 的连接点）。把它换成
//!    指向实例目录的连接点会触发 `AppModel-State` 的「修复状态位置」，连接点被回滚，
//!    同时 `InitializeDataChangedSignaler` 报 `E_ACCESSDENIED`。
//!
//! # 隔离怎么做（本条决定隔离是否真的成立）
//!
//! 只替换 `LocalState` **里面的子目录**：`games` / `logs` / `crash` / `premium_cache`
//! / `speech` 分别做成指向 `<实例目录>/…/<同名>` 的连接点。系统只托管状态根，不碰子目录，
//! 实测不会回滚；游戏数据（世界、资源包、行为包、皮肤、截图、日志、崩溃转储）因此
//! 全部落在实例目录里。
//!
//! 还有一条：AppContainer 令牌是**写受限**的（`WRITE_RESTRICTED`），写权限只认令牌
//! 里的包 SID / 能力 SID，不认 `Authenticated Users`。所以实例数据目录必须带上与
//! 包状态目录**同一套 DACL**，否则游戏读得到、写不进（实测：不给 ACL 时 40 秒内
//! `games/` 0 个文件；给了 ACL 立刻出现 `games/com.mojang/...`）。
//! 这里直接复制 `LocalState` 的 DACL，而不是自己拼 ACE：包需要哪些 SID 由系统登记，
//! 猜错一个就是「能启动但存不了档」这种最难查的故障。

use std::path::{Path, PathBuf};

use super::uwp::{PackageManifest, UwpError, APPX_MANIFEST};

/// 会被重定向到实例目录的包状态子目录。
///
/// 只列游戏自己创建的目录：`LocalState` 根下的配置 JSON（`global_settings.json` 等）
/// 与系统托管的其它状态目录（`AppData` / `Settings` / `AC` / `SystemAppData`）不动，
/// 它们是包级状态，替换它们会碰到上面第 3 条约束。
pub const REDIRECTED_DATA_DIRS: &[&str] = &[
    "games",
    "logs",
    "crash",
    "premium_cache",
    "speech",
    "treatments",
    "Flighting",
];

/// 已注册到系统的包（只保留调用方需要的字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredPackage {
    /// `PackageFullName`，如 `Microsoft.MinecraftUWP_1.21.11401.0_x64__8wekyb3d8bbwe`。
    pub full_name: String,
    /// `PackageFamilyName`，如 `Microsoft.MinecraftUWP_8wekyb3d8bbwe`。
    pub family_name: String,
    /// 包内容所在的目录（松散注册时就是实例目录）。
    pub install_location: PathBuf,
    /// 是否开发模式（松散）注册。商店安装的包为 `false`。
    pub development_mode: bool,
}

/// 一次数据目录重定向的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedirectReport {
    /// 包状态根（`...\Packages\<族名>\LocalState`）。
    pub state_dir: PathBuf,
    /// 已指向实例目录的子目录。
    pub redirected: Vec<PathBuf>,
    /// 原有的真实目录被整体让位到这里（保留用户数据，不删除）。
    pub moved_aside: Vec<PathBuf>,
}

/// 包状态根（`%LOCALAPPDATA%\Packages\<族名>\LocalState`）。
///
/// 它本身可能是系统建的连接点（安装卷非系统盘时指向 `WpSystem`），这里按**路径**用，
/// 不解析连接点：穿过连接点写到真正的状态目录里，正是我们想要的（见模块注释第 3 条）。
pub fn package_state_dir(family_name: &str) -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Packages")
        .join(family_name)
        .join("LocalState")
}

// ---------------------------------------------------------------- Windows
#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::fs::MetadataExt;
    use std::time::{Duration, Instant};

    use windows::core::{GUID, HSTRING, Interface, PCWSTR};
    use windows::ApplicationModel::Package;
    use windows::Foundation::{AsyncStatus, IAsyncInfo, Uri};
    use windows::Management::Deployment::{
        DeploymentProgress, DeploymentResult, PackageManager, RegisterPackageOptions,
        RemovalOptions,
    };
    use windows::Win32::Foundation::{CloseHandle, LocalFree, GENERIC_WRITE, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_NONE,
        OPEN_EXISTING,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
    use windows::Win32::UI::Shell::{ACTIVATEOPTIONS, IApplicationActivationManager};

    /// `IApplicationActivationManager` 的 CLSID（shobjidl / twinui）。
    const ACTIVATION_MANAGER_CLSID: GUID =
        GUID::from_u128(0x45BA127D_10A8_46EA_8AB7_56EA9078943C);
    /// 连接点重解析标记（`IO_REPARSE_TAG_MOUNT_POINT`）。
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    /// `FSCTL_SET_REPARSE_POINT`（`CTL_CODE(FILE_DEVICE_FILE_SYSTEM=9, 41, METHOD_BUFFERED, FILE_WRITE_DATA=2)`）。
    const FSCTL_SET_REPARSE_POINT: u32 = (9 << 16) | (2 << 14) | (41 << 2);
    /// 文件属性：重解析点。
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    /// 注册 / 卸载的等待上限（GB 级松散注册在本机实测约 10 s，留足余量）。
    const DEPLOYMENT_TIMEOUT: Duration = Duration::from_secs(300);

    #[link(name = "kernel32")]
    extern "system" {
        fn DeviceIoControl(
            hdevice: HANDLE,
            dwiocontrolcode: u32,
            lpinbuffer: *const core::ffi::c_void,
            ninbuffersize: u32,
            lpoutbuffer: *mut core::ffi::c_void,
            noutbuffersize: u32,
            lpbytesreturned: *mut u32,
            lpoverlapped: *mut core::ffi::c_void,
        ) -> i32;
    }

    /// 在独立线程里跑一段需要 COM / WinRT 初始化的代码。
    ///
    /// 必须在独立线程：`RoInitialize` / `CoInitializeEx` 是**线程级**状态，而调用方
    /// （Tauri 命令、tokio 阻塞线程池）的线程由框架复用，谁都没资格替别人定 apartment。
    fn in_runtime<T, F>(apartment: Apartment, work: F) -> Result<T, UwpError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, UwpError> + Send + 'static,
    {
        let handle = std::thread::Builder::new()
            .name("copper-uwp-deploy".into())
            .spawn(move || {
                // SAFETY: init / uninit 在同一线程内配对。
                match apartment {
                    Apartment::MultiThreaded => {
                        let hr = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
                        if hr.is_err() {
                            return Err(UwpError::Platform(format!("RoInitialize 失败: {hr:?}")));
                        }
                    }
                    Apartment::SingleThreaded => {
                        // 已初始化（S_FALSE）与既有 apartment 冲突（RPC_E_CHANGED_MODE）
                        // 都不能在这里当失败：后者说明线程已被初始化过，后续调用自会
                        // 给出它自己的结论，比在这里臆断失败更好排查。
                        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                    }
                }
                let result = work();
                match apartment {
                    Apartment::MultiThreaded => unsafe { RoUninitialize() },
                    Apartment::SingleThreaded => unsafe { CoUninitialize() },
                }
                result
            })
            .map_err(|error| UwpError::Manifest(format!("无法创建部署线程: {error}")))?;
        handle
            .join()
            .map_err(|_| UwpError::Manifest("部署线程异常退出".into()))?
    }

    #[derive(Clone, Copy)]
    enum Apartment {
        MultiThreaded,
        SingleThreaded,
    }

    // ------------------------------------------------------------ 查询

    fn package_manager() -> Result<PackageManager, UwpError> {
        PackageManager::new()
            .map_err(|error| UwpError::Manifest(format!("无法创建 PackageManager: {error}")))
    }

    fn collect(iterable: windows::Foundation::Collections::IIterable<Package>) -> Result<Vec<Package>, UwpError> {
        let iterator = iterable
            .First()
            .map_err(|error| UwpError::Manifest(format!("枚举已安装包失败: {error}")))?;
        let mut out = Vec::new();
        loop {
            let has = iterator
                .HasCurrent()
                .map_err(|error| UwpError::Manifest(format!("枚举已安装包失败: {error}")))?;
            if !has {
                break;
            }
            out.push(
                iterator
                    .Current()
                    .map_err(|error| UwpError::Manifest(format!("读取已安装包失败: {error}")))?,
            );
            iterator
                .MoveNext()
                .map_err(|error| UwpError::Manifest(format!("枚举已安装包失败: {error}")))?;
        }
        Ok(out)
    }

    fn describe(package: &Package) -> Result<RegisteredPackage, UwpError> {
        let id = package
            .Id()
            .map_err(|error| UwpError::Manifest(format!("读取包标识失败: {error}")))?;
        let full_name = id
            .FullName()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        // 包族名在 `PackageId` 上（`Package` 本身没有该属性）。
        let family_name = id
            .FamilyName()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let install_location = package
            .InstalledLocation()
            .and_then(|folder| folder.Path())
            .map(|value| PathBuf::from(value.to_string_lossy()))
            .unwrap_or_default();
        let development_mode = package.IsDevelopmentMode().unwrap_or(false);
        Ok(RegisteredPackage {
            full_name,
            family_name,
            install_location,
            development_mode,
        })
    }

    /// 查询某身份名的已注册包（同名只返回一个：包族里同身份只会注册一个版本）。
    pub fn query(identity_name: &str) -> Result<Option<RegisteredPackage>, UwpError> {
        let key = identity_name.to_string();
        in_runtime(Apartment::MultiThreaded, move || {
            let manager = package_manager()?;
            let packages = collect(
                manager
                    .FindPackages()
                    .map_err(|error| UwpError::Manifest(format!("枚举已安装包失败: {error}")))?,
            )?;
            for package in packages {
                let info = describe(&package)?;
                let name = package
                    .Id()
                    .map(|id| id.Name().map(|value| value.to_string_lossy()))
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                if name.eq_ignore_ascii_case(&key) {
                    return Ok(Some(info));
                }
            }
            Ok(None)
        })
    }

    // ------------------------------------------------------------ 注册 / 卸载

    /// 等待一个部署异步操作完成。
    ///
    /// 部署 API 会阻塞数秒（松散注册本机实测约 10 s）。这里不用 `SetCompleted` 委托：
    /// 委托回调要跨线程投递，而本函数已经跑在独立线程上，轮询状态更简单也更好读。
    fn wait_deployment(
        operation: windows::Foundation::IAsyncOperationWithProgress<DeploymentResult, DeploymentProgress>,
        stage: &'static str,
    ) -> Result<(), UwpError> {
        let info: IAsyncInfo = operation
            .cast()
            .map_err(|error| UwpError::Manifest(format!("{stage}: 取异步状态失败: {error}")))?;
        let deadline = Instant::now() + DEPLOYMENT_TIMEOUT;
        loop {
            let status = info
                .Status()
                .map_err(|error| UwpError::Manifest(format!("{stage}: 取异步状态失败: {error}")))?;
            match status {
                AsyncStatus::Completed | AsyncStatus::Error | AsyncStatus::Canceled => {
                    // `GetResults` 在失败时返回部署层的 HRESULT 与错误文本，比
                    // `IAsyncInfo::ErrorCode` 更有信息量（后者只有一个码）。
                    return operation.GetResults().map(|_| ()).map_err(|error| {
                        UwpError::Manifest(format!("{stage} 失败: {}", describe_deployment_error(&error)))
                    });
                }
                _ => {}
            }
            if Instant::now() > deadline {
                return Err(UwpError::Manifest(format!("{stage} 超时")));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// 把部署错误里的细节抠出来（HRESULT 之外还有 `DeploymentResult::ErrorText`）。
    fn describe_deployment_error(error: &windows::core::Error) -> String {
        format!("{} (HRESULT 0x{:08X})", error.message(), error.code().0 as u32)
    }

    /// 松散注册一个实例目录。
    ///
    /// 会先校验目录里的清单与已注册状态，注册后再回读系统记录，确认**系统认的
    /// 就是这一份目录**——只看 API 返回成功是不够的：Windows 完全可能保留了旧版本的
    /// 注册（`0x80073CFB` 一类冲突），那时「注册成功」而实际指向另一个目录。
    pub fn register(install_dir: &Path) -> Result<RegisteredPackage, UwpError> {
        let dir = install_dir.to_path_buf();
        let manifest = PackageManifest::read_dir(&dir)?;
        manifest.validate()?;
        in_runtime(Apartment::MultiThreaded, move || {
            let manager = package_manager()?;
            let manifest_path = dir.join(APPX_MANIFEST);
            let uri = file_uri(&manifest_path)?;
            let options = RegisterPackageOptions::new()
                .map_err(|error| UwpError::Manifest(format!("创建注册选项失败: {error}")))?;
            options
                .SetForceUpdateFromAnyVersion(true)
                .map_err(|error| UwpError::Manifest(format!("设置注册选项失败: {error}")))?;
            let operation = manager
                .RegisterPackageByUriAsync(&uri, &options)
                .map_err(|error| {
                    UwpError::Manifest(format!(
                        "注册 UWP 包失败: {}",
                        describe_deployment_error(&error)
                    ))
                })?;
            wait_deployment(operation, "注册 UWP 包")?;

            // 回读校验：系统记录必须指向本次的实例目录与预期包族。
            let registered = query_in_runtime(&manager, &manifest.identity_name)?;
            let Some(package) = registered else {
                return Err(UwpError::Manifest(
                    "注册后系统里查不到该包（注册未真正生效）".into(),
                ));
            };
            if !same_path(&package.install_location, &dir) {
                return Err(UwpError::Manifest(format!(
                    "注册后包目录不是该实例：系统记录 `{}`，期望 `{}`",
                    package.install_location.display(),
                    dir.display()
                )));
            }
            if !package
                .family_name
                .eq_ignore_ascii_case(manifest.family_name())
            {
                return Err(UwpError::Manifest(format!(
                    "注册后包族不匹配：`{}`",
                    package.family_name
                )));
            }
            if !package.development_mode {
                return Err(UwpError::Manifest(
                    "注册结果不是开发模式包（松散注册未生效）".into(),
                ));
            }
            Ok(package)
        })
    }

    fn query_in_runtime(
        manager: &PackageManager,
        identity_name: &str,
    ) -> Result<Option<RegisteredPackage>, UwpError> {
        let packages = collect(
            manager
                .FindPackages()
                .map_err(|error| UwpError::Manifest(format!("枚举已安装包失败: {error}")))?,
        )?;
        for package in packages {
            let name = package
                .Id()
                .map(|id| id.Name().map(|value| value.to_string_lossy()))
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            if name.eq_ignore_ascii_case(identity_name) {
                return Ok(Some(describe(&package)?));
            }
        }
        Ok(None)
    }

    /// 若该目录正是系统登记的松散包，则注销它（保留应用数据）。
    ///
    /// 删除实例前必须调用：注册记录指向的是实例目录，目录被删掉后这条注册会变成
    /// 「指向不存在目录的包」，此后任何一次 `Add-AppxPackage`/激活都会以难以理解的
    /// 0x80073CFB / 0x80073D02 收场。
    pub fn unregister(install_dir: &Path) -> Result<bool, UwpError> {
        let dir = install_dir.to_path_buf();
        let manifest = PackageManifest::read_dir(&dir)?;
        in_runtime(Apartment::MultiThreaded, move || {
            let manager = package_manager()?;
            let Some(package) = query_in_runtime(&manager, &manifest.identity_name)? else {
                return Ok(false);
            };
            if !same_path(&package.install_location, &dir) {
                // 系统里的注册不是这个目录：不能替用户注销别人的包。
                return Ok(false);
            }
            let operation = manager
                .RemovePackageWithOptionsAsync(
                    &HSTRING::from(package.full_name.clone()),
                    RemovalOptions::PreserveApplicationData,
                )
                .map_err(|error| {
                    UwpError::Manifest(format!(
                        "注销 UWP 包失败: {}",
                        describe_deployment_error(&error)
                    ))
                })?;
            wait_deployment(operation, "注销 UWP 包")?;
            Ok(true)
        })
    }

    fn file_uri(path: &Path) -> Result<Uri, UwpError> {
        let url = reqwest::Url::from_file_path(path).map_err(|()| {
            UwpError::Manifest(format!("无法把路径转成 file URI: {}", path.display()))
        })?;
        Uri::CreateUri(&HSTRING::from(url.as_str()))
            .map_err(|error| UwpError::Manifest(format!("创建 file URI 失败: {error}")))
    }

    // ------------------------------------------------------------ 激活

    /// 通过 `IApplicationActivationManager` 激活已注册的包，返回游戏进程 pid。
    ///
    /// 走的是系统激活（不是 `CreateProcess`）：包身份、`ApplicationData` 与商店安装
    /// 完全一致。启动参数对 UWP 无效（系统按 `Application` 条目拉起），UWP 实例因此
    /// 忽略启动参数与编辑器模式。
    pub fn activate(app_user_model_id: &str) -> Result<u32, UwpError> {
        let aumid = app_user_model_id.to_string();
        // 激活管理器是 shell 的 STA 组件：用 STA 线程（与手工探针一致）。
        in_runtime(Apartment::SingleThreaded, move || {
            unsafe {
                let manager: IApplicationActivationManager =
                    CoCreateInstance(&ACTIVATION_MANAGER_CLSID, None, CLSCTX_INPROC_SERVER)
                        .map_err(|error| {
                            UwpError::Manifest(format!("创建激活管理器失败: {error}"))
                        })?;
                let id = HSTRING::from(aumid);
                let arguments = HSTRING::new();
                manager
                    .ActivateApplication(&id, &arguments, ACTIVATEOPTIONS(0))
                    .map_err(|error| {
                        UwpError::Manifest(format!(
                            "激活 UWP 应用失败: {}",
                            describe_deployment_error(&error)
                        ))
                    })
            }
        })
    }

    // ------------------------------------------------------------ 数据目录重定向

    fn read_junction_target(path: &Path) -> Option<PathBuf> {
        std::fs::read_link(path).ok()
    }

    fn is_reparse_point(path: &Path) -> bool {
        std::fs::symlink_metadata(path)
            .map(|meta| meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            .unwrap_or(false)
    }

    fn same_path(left: &Path, right: &Path) -> bool {
        let normalize = |path: &Path| {
            let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            resolved
                .to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_lowercase()
        };
        normalize(left) == normalize(right)
    }

    /// 把一个目录变成指向目标的连接点（目录必须不存在或已清空）。
    fn create_junction(link: &Path, target: &Path) -> Result<(), UwpError> {
        std::fs::create_dir_all(link)?;
        let substitute = format!("\\??\\{}", target.display());
        let print = target.display().to_string();
        let substitute: Vec<u16> = substitute.encode_utf16().collect();
        let print: Vec<u16> = print.encode_utf16().collect();

        // REPARSE_DATA_BUFFER + MOUNT_POINT_REPARSE_BUFFER：头 8 字节（tag/len/reserved），
        // 四个 u16 偏移（相对 PathBuffer），随后是两段以 NUL 结尾的 UTF-16。
        let path_bytes = (substitute.len() + 1 + print.len() + 1) * 2;
        let data_length = 8 + path_bytes;
        let mut buffer = vec![0u8; 8 + data_length];
        buffer[0..4].copy_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        buffer[4..6].copy_from_slice(&(data_length as u16).to_le_bytes());
        let substitute_offset: u16 = 0;
        let substitute_length: u16 = (substitute.len() * 2) as u16;
        let print_offset: u16 = ((substitute.len() + 1) * 2) as u16;
        let print_length: u16 = (print.len() * 2) as u16;
        buffer[8..10].copy_from_slice(&substitute_offset.to_le_bytes());
        buffer[10..12].copy_from_slice(&substitute_length.to_le_bytes());
        buffer[12..14].copy_from_slice(&print_offset.to_le_bytes());
        buffer[14..16].copy_from_slice(&print_length.to_le_bytes());
        let mut cursor = 16;
        for unit in &substitute {
            buffer[cursor..cursor + 2].copy_from_slice(&unit.to_le_bytes());
            cursor += 2;
        }
        cursor += 2; // NUL
        for unit in &print {
            buffer[cursor..cursor + 2].copy_from_slice(&unit.to_le_bytes());
            cursor += 2;
        }

        let link_wide = wide(link);
        // SAFETY: 句柄在本函数内关闭；缓冲区在调用期间保持存活。
        unsafe {
            let handle = CreateFileW(
                PCWSTR(link_wide.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                None,
            )
            .map_err(|error| {
                UwpError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("打开连接点目标失败 {}: {error}", link.display()),
                ))
            })?;
            let mut returned = 0u32;
            let ok = DeviceIoControl(
                handle,
                FSCTL_SET_REPARSE_POINT,
                buffer.as_ptr() as *const core::ffi::c_void,
                buffer.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            );
            let last_error = windows::core::Error::from_win32();
            let _ = CloseHandle(handle);
            if ok == 0 {
                let _ = std::fs::remove_dir(link);
                return Err(UwpError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("创建连接点失败 {}: {last_error}", link.display()),
                )));
            }
        }
        Ok(())
    }

    /// 摘掉连接点（只删链接，目标目录原样保留）。
    fn remove_junction(link: &Path) -> Result<(), UwpError> {
        std::fs::remove_dir(link).map_err(|error| {
            UwpError::Io(std::io::Error::new(
                error.kind(),
                format!("删除连接点失败 {}: {error}", link.display()),
            ))
        })
    }

    fn wide(path: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// 复制包状态目录的 DACL 到实例数据目录。
    ///
    /// 为什么不是「自己拼一条 ACE」：AppContainer 的写权限由包登记决定（包 SID +
    /// 能力 SID），这些 SID 的确切集合只有系统知道。直接把 `LocalState` 的 DACL
    /// 抄过来，等于「让实例目录对游戏拥有与它自己的状态目录完全相同的权限」。
    fn inherit_package_acl(state_dir: &Path, target_dir: &Path) -> Result<(), UwpError> {
        let state_wide = wide(state_dir);
        let target_wide = wide(target_dir);
        unsafe {
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut descriptor = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
            let status = GetNamedSecurityInfoW(
                PCWSTR(state_wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&mut dacl),
                None,
                &mut descriptor,
            );
            if status.0 != 0 || dacl.is_null() {
                return Err(UwpError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!(
                        "读取包状态目录的 ACL 失败 {}（{}）",
                        state_dir.display(),
                        status.0
                    ),
                )));
            }
            // `GetNamedSecurityInfoW` 返回的 ACL 归 descriptor 所有，调用结束后释放。
            // 目标对象在 `SetNamedSecurityInfoW` 里会复制一份，不共享所有权。
            let null_sid = PSID(std::ptr::null_mut());
            let set = SetNamedSecurityInfoW(
                PCWSTR(target_wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_sid,
                null_sid,
                Some(dacl),
                None,
            );
            let _ = LocalFree(HLOCAL(descriptor.0));
            if set.0 != 0 {
                return Err(UwpError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!(
                        "设置实例数据目录 ACL 失败 {}（{}）",
                        target_dir.display(),
                        set.0
                    ),
                )));
            }
        }
        Ok(())
    }

    /// 把包状态下的游戏数据子目录重定向到实例数据目录。
    ///
    /// 幂等：连接点已经把目标指对时零写入；真实目录（包刚被游戏跑过一次时会出现）
    /// 整体让位到 `<名>.copper-<时间戳>` 而不删除——那是用户数据，删了就找不回来。
    pub fn redirect_data_dirs(family_name: &str, data_dir: &Path) -> Result<RedirectReport, UwpError> {
        std::fs::create_dir_all(data_dir)?;
        let state_dir = package_state_dir(family_name);
        if !state_dir.is_dir() {
            return Err(UwpError::Manifest(format!(
                "包状态目录不存在：{}（实例可能尚未注册）",
                state_dir.display()
            )));
        }
        // 先给实例目录一份与包状态目录一致的 ACL，再建连接点：顺序反了的话，
        // 游戏在第一次写入时就会因为没有写权限而静默失败。
        inherit_package_acl(&state_dir, data_dir)?;

        let mut report = RedirectReport {
            state_dir: state_dir.clone(),
            ..Default::default()
        };
        for name in REDIRECTED_DATA_DIRS {
            let link = state_dir.join(name);
            let target = data_dir.join(name);
            std::fs::create_dir_all(&target)?;
            if is_reparse_point(&link) {
                let current = read_junction_target(&link);
                if current.as_deref().map(|p| same_path(p, &target)).unwrap_or(false) {
                    inherit_package_acl(&state_dir, &target)?;
                    report.redirected.push(target);
                    continue;
                }
                remove_junction(&link)?;
            } else if link.is_dir() {
                // 真实目录：整体让位（不删用户数据），再建连接点。
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_secs())
                    .unwrap_or(0);
                let mut aside = state_dir.join(format!("{name}.copper-{stamp}"));
                let mut attempt = 0;
                while aside.exists() {
                    attempt += 1;
                    aside = state_dir.join(format!("{name}.copper-{stamp}-{attempt}"));
                }
                std::fs::rename(&link, &aside).map_err(|error| {
                    UwpError::Io(std::io::Error::new(
                        error.kind(),
                        format!(
                            "无法把已有的包数据目录让位（{} -> {}）: {error}",
                            link.display(),
                            aside.display()
                        ),
                    ))
                })?;
                log::warn!(
                    "[uwp] 包数据目录 `{}` 已整体让位到 `{}`（用户数据未删除）",
                    link.display(),
                    aside.display()
                );
                report.moved_aside.push(aside);
            } else if link.exists() {
                // 同名文件：不是我们该处理的东西，直接让位。
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_secs())
                    .unwrap_or(0);
                let aside = state_dir.join(format!("{name}.copper-{stamp}"));
                std::fs::rename(&link, &aside)?;
                report.moved_aside.push(aside);
            }
            create_junction(&link, &target)?;
            inherit_package_acl(&state_dir, &target)?;
            report.redirected.push(target);
        }
        Ok(report)
    }

    /// 断开所有指向某个实例数据目录的连接点。
    ///
    /// 删除实例前调用：留着悬空连接点会让「包状态目录」里出现一批指向已删除目录的
    /// 重解析点，任何写入（含游戏自己）都会以 `0xC0000098` 一类错误失败。
    pub fn clear_data_redirect(family_name: &str, data_dir: &Path) -> Result<usize, UwpError> {
        let state_dir = package_state_dir(family_name);
        if !state_dir.is_dir() {
            return Ok(0);
        }
        let mut removed = 0usize;
        for name in REDIRECTED_DATA_DIRS {
            let link = state_dir.join(name);
            if !is_reparse_point(&link) {
                continue;
            }
            let Some(target) = read_junction_target(&link) else {
                continue;
            };
            if !same_path(&target, &data_dir.join(name)) {
                continue;
            }
            remove_junction(&link)?;
            removed += 1;
        }
        Ok(removed)
    }
}

// ---------------------------------------------------------------- 非 Windows 存根
#[cfg(not(windows))]
mod imp {
    use super::*;

    fn unsupported<T>() -> Result<T, UwpError> {
        Err(UwpError::UnsupportedPlatform)
    }

    pub fn query(_identity_name: &str) -> Result<Option<RegisteredPackage>, UwpError> {
        unsupported()
    }
    pub fn register(_install_dir: &Path) -> Result<RegisteredPackage, UwpError> {
        unsupported()
    }
    pub fn unregister(_install_dir: &Path) -> Result<bool, UwpError> {
        unsupported()
    }
    pub fn activate(_app_user_model_id: &str) -> Result<u32, UwpError> {
        unsupported()
    }
    pub fn redirect_data_dirs(
        _family_name: &str,
        _data_dir: &Path,
    ) -> Result<RedirectReport, UwpError> {
        unsupported()
    }
    pub fn clear_data_redirect(_family_name: &str, _data_dir: &Path) -> Result<usize, UwpError> {
        unsupported()
    }
}

pub use imp::{activate, clear_data_redirect, query, redirect_data_dirs, register, unregister};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dir_is_under_local_packages() {
        let dir = package_state_dir("Microsoft.MinecraftUWP_8wekyb3d8bbwe");
        assert!(dir.ends_with(
            PathBuf::from("Packages")
                .join("Microsoft.MinecraftUWP_8wekyb3d8bbwe")
                .join("LocalState")
        ));
    }

    #[test]
    fn redirected_data_dirs_are_unique_and_non_empty() {
        assert!(!REDIRECTED_DATA_DIRS.is_empty());
        let mut seen = std::collections::HashSet::new();
        for name in REDIRECTED_DATA_DIRS {
            assert!(!name.trim().is_empty());
            assert!(!name.contains(['/', '\\']), "子目录名不能带分隔符: {name}");
            assert!(seen.insert(name.to_lowercase()), "重复子目录: {name}");
        }
        // 世界与内容目录必须在 `games` 之下（这是重定向能覆盖到它们的唯一前提）。
        assert!(REDIRECTED_DATA_DIRS.contains(&"games"));
        assert!(REDIRECTED_DATA_DIRS.contains(&"logs"));
    }
}
