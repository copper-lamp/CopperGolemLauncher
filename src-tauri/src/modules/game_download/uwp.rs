//! UWP 安装包（`.appx` / `.appxbundle`）的解析与解包。
//!
//! # 与 GDK 解包的区别
//!
//! | | GDK（`.msixvc`） | UWP（`.appx`） |
//! |---|---|---|
//! | 容器 | 加密 XVC（AES-XTS 逐页） | 普通 ZIP |
//! | 授权 | 需要商店 content key | **不需要**（包未加密） |
//! | 身份 | `MicrosoftGame.config` | `AppxManifest.xml` |
//! | 产物校验 | `Minecraft.Windows.exe` + `MicrosoftGame.config` | 清单里的可执行文件 + x64 PE |
//!
//! 所以 UWP 不解密、不碰商店授权，但必须**严格校验包身份**：解出来的东西要交给系统
//! 松散注册，一旦是别的应用（或改过身份的仿冒包），注册后指向的是另一个程序。
//!
//! # 安全边界
//!
//! UWP 包来自微软 CDN，但仍按不可信输入处理：
//! - 条目路径必须是包内相对路径：拒绝盘符、UNC、`..`、控制字符、ADS（`:`）、
//!   保留设备名（`CON`/`NUL`/`COM1`…）、结尾空格或点；
//! - 大小写折叠后重复的条目拒绝（Windows 文件系统不区分大小写，两个条目写同一路径）；
//! - 单个条目必须真的写满声明长度，总量与条目数都有上限；
//! - `AppxSignature.p7x` 不解（松散注册不校验签名，留着只会在解包目录里多一个
//!   无法验证的二进制）；
//! - 解包到同级 staging 目录再整体 rename 发布，失败不留半成品（由调用方负责删）。

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::Reader;

use super::uwp_catalog::{PREVIEW_IDENTITY, RELEASE_IDENTITY};

/// 微软发行者名（`Identity/@Publisher`），逐字比对。
///
/// 只校验名称不足以说明「这是微软的包」：把 `Identity/@Name` 改成 Minecraft 的仿冒包
/// 可以用同一个名字，发行者名是配对校验里的第二把锁。
pub const MICROSOFT_PUBLISHER: &str =
    "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US";

/// 单个包解包后的总字节上限（20 GiB）。
const MAX_TOTAL_BYTES: u64 = 20 << 30;
/// 条目数上限（当前真实包约 3.3 万条，留足余量）。
const MAX_ENTRIES: usize = 100_000;
/// 清单文件读取上限（真实清单约 20 KiB）。
const MAX_MANIFEST_BYTES: u64 = 4 << 20;
/// 包内清单文件名。
pub const APPX_MANIFEST: &str = "AppxManifest.xml";
/// 包内签名文件名（不解）。
pub const APPX_SIGNATURE: &str = "AppxSignature.p7x";
/// 包束清单（`.appxbundle` 才有）。
const BUNDLE_MANIFEST: &str = "AppxMetadata/AppxBundleManifest.xml";

/// UWP 处理错误。
#[derive(Debug, thiserror::Error)]
pub enum UwpError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("安装包解析错误: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("安装包清单缺失或无法解析: {0}")]
    Manifest(String),
    #[error("该包不是 Minecraft：{0}")]
    NotMinecraft(String),
    #[error("安装包条目路径不安全: {0}")]
    UnsafeEntry(String),
    #[error("安装包体积或条目数超出上限")]
    TooLarge,
    #[error("安装包目标架构不受支持：{0}")]
    UnsupportedArchitecture(String),
    #[error("解包产物缺少游戏主程序")]
    MissingExecutable,
    #[error("UWP 安装只在 Windows 上可用")]
    UnsupportedPlatform,
    /// 平台侧（COM / WinRT / 文件系统）失败，附带可读步骤名。
    #[error("UWP 平台调用失败: {0}")]
    Platform(String),
}

impl From<UwpError> for crate::error::KernelError {
    fn from(error: UwpError) -> Self {
        crate::error::KernelError::Config(format!("UWP 安装包处理失败: {error}"))
    }
}

// ---------------------------------------------------------------- 清单

/// `AppxManifest.xml` 里安装流程真正需要的那几项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageManifest {
    /// `Identity/@Name`，如 `Microsoft.MinecraftUWP`。
    pub identity_name: String,
    /// `Identity/@Publisher`。
    pub publisher: String,
    /// `Identity/@Version`（包版本，如 `1.21.11401.0`）。
    pub package_version: String,
    /// `Identity/@ProcessorArchitecture`，如 `x64` / `neutral`。
    pub architecture: String,
    /// 首个 `Application/@Id`（激活时拼 `族名!Id`）。
    pub application_id: String,
    /// 首个 `Application/@Executable`（相对包根）。
    pub executable: String,
}

impl PackageManifest {
    /// 读实例目录里的清单。
    pub fn read_dir(dir: &Path) -> Result<Self, UwpError> {
        let path = dir.join(APPX_MANIFEST);
        let bytes = read_bounded(&path, MAX_MANIFEST_BYTES)?;
        Self::parse(&bytes)
    }

    /// 从字节解析（可单测）。
    pub fn parse(bytes: &[u8]) -> Result<Self, UwpError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| UwpError::Manifest(format!("清单不是 UTF-8 文本: {e}")))?;
        let mut reader = Reader::from_str(text);
        reader.config_mut().trim_text(true);

        let mut identity: Option<(String, String, String, String)> = None;
        let mut application: Option<(String, String)> = None;
        // 只在 Application 的直接父级是 Applications 时取第一项：Extensions 里也有
        // 名为 `Application` 的扩展节点，按名字盲取会拿错可执行文件。
        let mut stack: Vec<String> = Vec::new();

        loop {
            match reader.read_event() {
                Ok(Event::Start(element)) => {
                    let local = local_name(String::from_utf8_lossy(element.name().as_ref()).as_ref());
                    if local == "Package" && stack.is_empty() {
                        stack.push(local.clone());
                    } else if local == "Identity" && identity.is_none() {
                        identity = Some((
                            attr(&element, "Name"),
                            attr(&element, "Publisher"),
                            attr(&element, "Version"),
                            attr(&element, "ProcessorArchitecture"),
                        ));
                        stack.push(local.clone());
                    } else if local == "Application"
                        && application.is_none()
                        && stack.last().map(String::as_str) == Some("Applications")
                    {
                        application = Some((attr(&element, "Id"), attr(&element, "Executable")));
                        stack.push(local.clone());
                    } else {
                        stack.push(local);
                    }
                }
                Ok(Event::Empty(element)) => {
                    let local = local_name(String::from_utf8_lossy(element.name().as_ref()).as_ref());
                    if local == "Identity" && identity.is_none() {
                        identity = Some((
                            attr(&element, "Name"),
                            attr(&element, "Publisher"),
                            attr(&element, "Version"),
                            attr(&element, "ProcessorArchitecture"),
                        ));
                    } else if local == "Application"
                        && application.is_none()
                        && stack.last().map(String::as_str) == Some("Applications")
                    {
                        application = Some((attr(&element, "Id"), attr(&element, "Executable")));
                    }
                }
                Ok(Event::End(_)) => {
                    stack.pop();
                }
                Ok(Event::Eof) => break,
                Err(error) => {
                    return Err(UwpError::Manifest(format!("XML 解析失败: {error}")));
                }
                _ => {}
            }
        }

        let (identity_name, publisher, package_version, architecture) = identity
            .ok_or_else(|| UwpError::Manifest("缺少 Identity 元素".into()))?;
        let (application_id, executable) = application
            .ok_or_else(|| UwpError::Manifest("缺少 Applications/Application 元素".into()))?;

        Ok(Self {
            identity_name,
            publisher,
            package_version,
            architecture,
            application_id,
            executable,
        })
    }

    /// 校验「这是微软的 Minecraft UWP 包」。
    pub fn validate(&self) -> Result<(), UwpError> {
        if !self.identity_name.eq_ignore_ascii_case(RELEASE_IDENTITY)
            && !self.identity_name.eq_ignore_ascii_case(PREVIEW_IDENTITY)
        {
            return Err(UwpError::NotMinecraft(format!(
                "包身份 `{}` 不是 Minecraft",
                self.identity_name
            )));
        }
        if !self.publisher.eq_ignore_ascii_case(MICROSOFT_PUBLISHER) {
            return Err(UwpError::NotMinecraft(format!(
                "发行者不是微软: `{}`",
                self.publisher
            )));
        }
        // 包版本必须是四段 u16：激活、版本号换算、隔离目录名都依赖它。
        let parts: Vec<&str> = self.package_version.split('.').collect();
        if parts.len() != 4
            || !parts
                .iter()
                .all(|part| !part.is_empty() && part.parse::<u16>().is_ok())
        {
            return Err(UwpError::Manifest(format!(
                "包版本号非法: `{}`",
                self.package_version
            )));
        }
        if self.application_id.trim().is_empty()
            || self.application_id.contains(['!', '\\', '/', '\0'])
        {
            return Err(UwpError::Manifest(format!(
                "Application/@Id 非法: `{}`",
                self.application_id
            )));
        }
        safe_relative_path(&self.executable)?;
        // 主程序名白名单：清单可以指向包内任意相对路径，但它必须真的是游戏本体。
        let exe_name = file_name_of(&self.executable);
        if !exe_name.eq_ignore_ascii_case("Minecraft.Windows.exe")
            && !exe_name.eq_ignore_ascii_case("Minecraft.Win10.DX11.exe")
        {
            return Err(UwpError::NotMinecraft(format!(
                "可执行文件不是 Minecraft 主程序: `{}`",
                self.executable
            )));
        }
        if arch_rank(&self.architecture).is_none() {
            return Err(UwpError::UnsupportedArchitecture(self.architecture.clone()));
        }
        Ok(())
    }

    /// 是否为预览包身份。
    pub fn is_preview(&self) -> bool {
        self.identity_name.eq_ignore_ascii_case(PREVIEW_IDENTITY)
    }

    /// 包族名（`%LOCALAPPDATA%\Packages\<族名>`）。
    ///
    /// 包族名 = `Identity/@Name` + `_` + 发行者哈希（`8wekyb3d8bbwe` 是微软证书的
    /// 固定哈希）。这里不用自己算：两个身份的族名是常量，且必须与系统注册后的
    /// `PackageFamilyName` 逐字一致（激活时拼 `族名!AppId`）。
    pub fn family_name(&self) -> &'static str {
        if self.is_preview() {
            super::uwp_catalog::PREVIEW_FAMILY
        } else {
            super::uwp_catalog::RELEASE_FAMILY
        }
    }

    /// 激活用的 AUMID（`族名!ApplicationId`）。
    pub fn app_user_model_id(&self) -> String {
        format!("{}!{}", self.family_name(), self.application_id)
    }

    /// 数值游戏版本号（`1.21.11401.0` → `1.21.114.1`）。
    ///
    /// 商店包版本编码规则（与 LeviLauncher `Manifest.GameVersion` 一致）：
    /// - `1.x` 起：第三段 = `patch * 100 + revision`，如 `1.21.11401.0` → `1.21.114.1`；
    /// - 早期 `0.x`：第二段 = `minor * 10 + patch`，如 `0.140.1.0` → `0.14.0.1`。
    pub fn game_version(&self) -> Option<String> {
        let mut numbers = [0u32; 4];
        for (index, part) in self.package_version.split('.').enumerate() {
            if index > 3 {
                return None;
            }
            numbers[index] = part.parse().ok()?;
        }
        if numbers[0] == 0 && numbers[1] >= 100 {
            return Some(format!(
                "0.{}.{}.{}",
                numbers[1] / 10,
                numbers[1] % 10,
                numbers[2]
            ));
        }
        Some(format!(
            "{}.{}.{}.{}",
            numbers[0],
            numbers[1],
            numbers[2] / 100,
            numbers[2] % 100
        ))
    }
}

/// XML 本地名（去掉命名空间前缀）。
fn local_name(raw: &str) -> String {
    raw.rsplit(':').next().unwrap_or(raw).to_string()
}

/// 取元素属性值（忽略命名空间前缀，属性名大小写不敏感）。
fn attr(element: &quick_xml::events::BytesStart<'_>, name: &str) -> String {
    for attribute in element.attributes().flatten() {
        let key = std::str::from_utf8(attribute.key.as_ref()).unwrap_or_default();
        if local_name(key).eq_ignore_ascii_case(name) {
            return String::from_utf8_lossy(&attribute.value).into_owned();
        }
    }
    String::new()
}

/// 路径里的文件名部分（同时接受 `/` 与 `\`）。
fn file_name_of(rel: &str) -> &str {
    rel.rsplit(['/', '\\']).next().unwrap_or(rel)
}

/// 架构优先级：x64 最佳，x86 次之（不足取），neutral 最后。
fn arch_rank(architecture: &str) -> Option<u8> {
    match architecture.trim().to_ascii_lowercase().as_str() {
        "x64" | "amd64" => Some(0),
        "x86" => Some(1),
        "neutral" => Some(2),
        _ => None,
    }
}

// ---------------------------------------------------------------- 解包

/// 一个包束里的应用包描述。
#[derive(Debug, Clone, PartialEq, Eq)]
struct BundlePackage {
    kind: String,
    architecture: String,
    file_name: String,
}

/// 解包 UWP 安装包到 `out_dir`，返回校验通过的清单。
///
/// `on_progress(已完成条目, 总条目, 当前相对路径)`：单元是**条目数**，与 GDK 侧
/// 解包进度同一口径（ZIP 条目数），由调用方换算成阶段内进度。
///
/// `out_dir` 必须不存在或为空：调用方（安装流水线）在调用前清理半成品目录，
/// 这里不再容忍「往旧目录里叠文件」——那会留下上一次失败安装的残留。
pub fn extract_package<F>(
    src: &Path,
    out_dir: &Path,
    on_progress: &mut F,
) -> Result<PackageManifest, UwpError>
where
    F: FnMut(u64, u64, &str),
{
    std::fs::create_dir_all(out_dir)?;
    let file = std::fs::File::open(src)?;
    let mut bundle = zip::ZipArchive::new(file)?;

    // 直接是应用包（含 AppxManifest.xml）还是包束（含 AppxBundleManifest.xml）。
    let mut has_manifest = false;
    for index in 0..bundle.len() {
        let entry = bundle.by_index(index)?;
        if entry.name().replace('\\', "/").eq_ignore_ascii_case(APPX_MANIFEST) {
            has_manifest = true;
            break;
        }
    }
    if has_manifest {
        extract_zip_entries(&mut bundle, out_dir, on_progress)?;
    } else {
        extract_from_bundle(src, &mut bundle, out_dir, on_progress)?;
    }

    let manifest = PackageManifest::read_dir(out_dir)?;
    manifest.validate()?;
    // 清单声明的可执行文件必须真的解出来了，且是 x64 PE。
    let exe_rel = safe_relative_path(&manifest.executable)?;
    let exe_path = out_dir.join(&exe_rel);
    if !exe_path.is_file() {
        return Err(UwpError::MissingExecutable);
    }
    match super::extractor::pe_machine(&exe_path) {
        Some(0x8664) => {}
        _ => return Err(UwpError::MissingExecutable),
    }
    Ok(manifest)
}

/// 从包束里挑出 x64 应用包，写临时文件后解包。
fn extract_from_bundle<F>(
    src: &Path,
    bundle: &mut zip::ZipArchive<std::fs::File>,
    out_dir: &Path,
    on_progress: &mut F,
) -> Result<(), UwpError>
where
    F: FnMut(u64, u64, &str),
{
    let mut bundle_manifest: Option<Vec<u8>> = None;
    for index in 0..bundle.len() {
        let mut entry = bundle.by_index(index)?;
        if entry
            .name()
            .replace('\\', "/")
            .eq_ignore_ascii_case(BUNDLE_MANIFEST)
        {
            if entry.size() > MAX_MANIFEST_BYTES {
                return Err(UwpError::TooLarge);
            }
            let mut raw = Vec::new();
            entry.read_to_end(&mut raw)?;
            bundle_manifest = Some(raw);
            break;
        }
    }
    let raw = bundle_manifest.ok_or_else(|| {
        UwpError::Manifest("既没有 AppxManifest.xml 也没有 AppxBundleManifest.xml".into())
    })?;
    let packages = parse_bundle_packages(&raw)?;
    // 独立资源包（语言 / 缩放）不参与松散注册：带着它注册会让身份变成资源包。
    if let Some(resource) = packages
        .iter()
        .find(|package| package.kind.eq_ignore_ascii_case("resource"))
    {
        return Err(UwpError::Manifest(format!(
            "包束里含独立资源包 `{}`，无法作为单个应用包安装",
            resource.file_name
        )));
    }
    let mut best: Option<(&BundlePackage, u8)> = None;
    for package in &packages {
        if !package.kind.eq_ignore_ascii_case("application") {
            continue;
        }
        let Some(rank) = arch_rank(&package.architecture) else {
            continue;
        };
        if best.map(|(_, current)| rank < current).unwrap_or(true) {
            best = Some((package, rank));
        }
    }
    let (selected, _) = best.ok_or_else(|| {
        UwpError::UnsupportedArchitecture("包束里没有 x64/x86 应用包".into())
    })?;
    let nested_rel = safe_relative_path(&selected.file_name)?;

    // 内嵌包写到包的同级目录：与源包同盘，避免把 GB 级临时文件扔进系统临时目录。
    let parent = out_dir.parent().unwrap_or(out_dir);
    let mut nested_path = parent.join(format!(
        ".copper-uwp-{}-{}.appx",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let mut attempt = 0;
    while nested_path.exists() {
        attempt += 1;
        nested_path = parent.join(format!(
            ".copper-uwp-{}-{attempt}.appx",
            std::process::id()
        ));
    }

    let write_nested = (|| -> Result<(), UwpError> {
        let mut selected_entry = None;
        for index in 0..bundle.len() {
            let entry = bundle.by_index(index)?;
            if entry.name().replace('\\', "/") == nested_rel.to_string_lossy().replace('\\', "/") {
                selected_entry = Some(index);
                break;
            }
        }
        let index = selected_entry.ok_or_else(|| {
            UwpError::Manifest(format!("包束清单里的 `{}` 不在包内", selected.file_name))
        })?;
        let mut entry = bundle.by_index(index)?;
        if entry.size() > MAX_TOTAL_BYTES {
            return Err(UwpError::TooLarge);
        }
        let mut out = std::fs::File::create(&nested_path)?;
        std::io::copy(&mut entry, &mut out)?;
        out.sync_all()?;
        Ok(())
    })();
    let result = write_nested.and_then(|()| {
        let file = std::fs::File::open(&nested_path)?;
        let mut nested = zip::ZipArchive::new(file)?;
        extract_zip_entries(&mut nested, out_dir, on_progress)
    });
    let _ = std::fs::remove_file(&nested_path);
    let _ = src;
    result
}

/// 解析包束清单里的 `<Package>` 节点。
fn parse_bundle_packages(raw: &[u8]) -> Result<Vec<BundlePackage>, UwpError> {
    let text = std::str::from_utf8(raw)
        .map_err(|e| UwpError::Manifest(format!("包束清单不是 UTF-8 文本: {e}")))?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut out = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                let name = std::str::from_utf8(element.name().as_ref())
                    .map(|raw| local_name(raw))
                    .unwrap_or_default();
                if name == "Package" {
                    out.push(BundlePackage {
                        kind: attr(&element, "Type"),
                        architecture: attr(&element, "Architecture"),
                        file_name: attr(&element, "FileName"),
                    });
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(UwpError::Manifest(format!("包束清单解析失败: {error}"))),
            _ => {}
        }
    }
    if out.is_empty() {
        return Err(UwpError::Manifest("包束清单里没有 Package 节点".into()));
    }
    Ok(out)
}

/// 按条目把 ZIP 内容解到目录，逐条做安全校验。
fn extract_zip_entries<F>(
    archive: &mut zip::ZipArchive<std::fs::File>,
    out_dir: &Path,
    on_progress: &mut F,
) -> Result<(), UwpError>
where
    F: FnMut(u64, u64, &str),
{
    if archive.len() > MAX_ENTRIES {
        return Err(UwpError::TooLarge);
    }
    let total = archive.len() as u64;

    // 第一遍：只做校验与总量统计（此时还没写任何文件，拒绝得便宜）。
    let mut planned: Vec<(usize, PathBuf, u64)> = Vec::with_capacity(archive.len());
    let mut seen: HashSet<String> = HashSet::new();
    let mut total_bytes: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let raw_name = entry.name().to_string();
        let normalized = raw_name.replace('\\', "/");
        if normalized.ends_with('/') || entry.is_dir() {
            continue;
        }
        if normalized.eq_ignore_ascii_case(APPX_SIGNATURE) {
            continue;
        }
        let rel = safe_relative_path(&normalized)?;
        // Windows 文件系统不区分大小写：两个条目折叠后同路径就是同一个文件。
        let key = rel.to_string_lossy().to_lowercase();
        if !seen.insert(key) {
            return Err(UwpError::UnsafeEntry(format!("重复条目 `{normalized}`")));
        }
        total_bytes = total_bytes
            .checked_add(entry.size())
            .ok_or(UwpError::TooLarge)?;
        if total_bytes > MAX_TOTAL_BYTES {
            return Err(UwpError::TooLarge);
        }
        planned.push((index, rel, entry.size()));
    }

    // 第二遍：写文件。目录与文件写前二次确认不越界（`safe_relative_path` 已保证）。
    let mut buffer = vec![0u8; 256 * 1024];
    for (done, (index, rel, declared)) in planned.into_iter().enumerate() {
        let mut entry = archive.by_index(index)?;
        on_progress(done as u64, total, &rel.to_string_lossy());
        let target = out_dir.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)?;
        let mut written: u64 = 0;
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            written += read as u64;
            if written > declared {
                return Err(UwpError::UnsafeEntry(format!(
                    "条目 `{}` 超出声明长度",
                    rel.display()
                )));
            }
            std::io::Write::write_all(&mut out, &buffer[..read])?;
        }
        if written != declared {
            return Err(UwpError::Zip(zip::result::ZipError::InvalidArchive(
                "条目长度与声明不符".into(),
            )));
        }
        out.sync_all()?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 路径安全

/// 归一化包内相对路径并做安全校验。
///
/// 返回可直接 `out_dir.join(..)` 的相对路径。规则与 MSIXVC 侧同一立场：
/// 绝对路径、盘符、UNC、`..`、控制字符、ADS、保留设备名、结尾空格/点一律拒绝。
pub fn safe_relative_path(name: &str) -> Result<PathBuf, UwpError> {
    let normalized = name.replace('\\', "/");
    // 尾部 `/` 表示目录条目，由调用方决定是否跳过；这里只去掉它再做词法校验，
    // 让 `a/b/` 与 `a/b` 得到同一个安全相对路径。
    let normalized = normalized.trim_end_matches('/');
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains(':')
        || normalized.contains('\0')
    {
        return Err(UwpError::UnsafeEntry(name.to_string()));
    }
    let mut out = PathBuf::new();
    let mut parts = 0usize;
    for component in normalized.split('/') {
        // 空段只可能来自 `a//b` 这类畸形路径：真包不会有，放行只会让「写到哪里」
        // 取决于文件系统怎么折叠，不如直接拒绝。
        if component.is_empty() {
            return Err(UwpError::UnsafeEntry(name.to_string()));
        }
        if component == "." {
            return Err(UwpError::UnsafeEntry(name.to_string()));
        }
        if component == ".." {
            return Err(UwpError::UnsafeEntry(name.to_string()));
        }
        if component.trim_end_matches([' ', '.']) != component {
            return Err(UwpError::UnsafeEntry(name.to_string()));
        }
        if component.bytes().any(|b| b < 32) {
            return Err(UwpError::UnsafeEntry(name.to_string()));
        }
        if component.contains(['<', '>', '"', '|', '?', '*']) {
            return Err(UwpError::UnsafeEntry(name.to_string()));
        }
        if is_reserved_name(component) {
            return Err(UwpError::UnsafeEntry(name.to_string()));
        }
        out.push(component);
        parts += 1;
    }
    if parts == 0 {
        return Err(UwpError::UnsafeEntry(name.to_string()));
    }
    Ok(out)
}

/// Windows 保留设备名（`CON` / `PRN` / `AUX` / `NUL` / `COM1..9` / `LPT1..9`）。
fn is_reserved_name(component: &str) -> bool {
    let base = component.split('.').next().unwrap_or(component).to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit()
            && base.as_bytes()[3] != b'0')
}

/// 读文件但限制长度（清单类小文件专用）。
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, UwpError> {
    let file = std::fs::File::open(path)?;
    let mut raw = Vec::new();
    file.take(limit + 1).read_to_end(&mut raw)?;
    if raw.len() as u64 > limit {
        return Err(UwpError::TooLarge);
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    const MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
         IgnorableNamespaces="uap">
  <Identity Name="Microsoft.MinecraftUWP"
            Publisher="CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US"
            Version="1.21.11401.0" ProcessorArchitecture="x64" />
  <Applications>
    <Application Id="App" Executable="Minecraft.Windows.exe" EntryPoint="Minecraft_Win10.App">
      <Extensions>
        <uap:Extension Category="windows.appService">
          <uap:AppService Name="x" />
        </uap:Extension>
      </Extensions>
    </Application>
  </Applications>
</Package>"#;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("copper_uwp_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 写一个最小 x64 PE（DOS 头 + PE 签名 + Machine=0x8664）。
    fn write_min_pe(path: &Path, machine: u16) {
        let mut pe = vec![0u8; 256];
        pe[0] = b'M';
        pe[1] = b'Z';
        pe[60] = 0x80;
        pe[0x80] = b'P';
        pe[0x81] = b'E';
        pe[0x84] = (machine & 0xff) as u8;
        pe[0x85] = (machine >> 8) as u8;
        std::fs::write(path, &pe).unwrap();
    }

    fn make_appx(path: &Path, manifest: &str, extra: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        zip.start_file(APPX_MANIFEST, opts).unwrap();
        zip.write_all(manifest.as_bytes()).unwrap();
        for (name, body) in extra {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.start_file(APPX_SIGNATURE, opts).unwrap();
        zip.write_all(b"signature").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn parses_real_manifest_shape() {
        let manifest = PackageManifest::parse(MANIFEST.as_bytes()).unwrap();
        assert_eq!(manifest.identity_name, "Microsoft.MinecraftUWP");
        assert_eq!(manifest.application_id, "App");
        assert_eq!(manifest.executable, "Minecraft.Windows.exe");
        assert_eq!(manifest.architecture, "x64");
        assert_eq!(manifest.game_version().as_deref(), Some("1.21.114.1"));
        assert_eq!(manifest.family_name(), "Microsoft.MinecraftUWP_8wekyb3d8bbwe");
        assert_eq!(
            manifest.app_user_model_id(),
            "Microsoft.MinecraftUWP_8wekyb3d8bbwe!App"
        );
        assert!(!manifest.is_preview());
        manifest.validate().unwrap();
    }

    #[test]
    fn game_version_follows_store_encoding() {
        let mut manifest = PackageManifest::parse(MANIFEST.as_bytes()).unwrap();
        // 1.x：第三段 = patch*100 + revision
        manifest.package_version = "1.21.9301.0".into();
        assert_eq!(manifest.game_version().as_deref(), Some("1.21.93.1"));
        // 0.x：第二段 = minor*10 + patch
        manifest.package_version = "0.140.1.0".into();
        assert_eq!(manifest.game_version().as_deref(), Some("0.14.0.1"));
        // 非法版本号
        manifest.package_version = "abc".into();
        assert!(manifest.game_version().is_none());
    }

    #[test]
    fn rejects_foreign_identity_or_publisher() {
        let mut manifest = PackageManifest::parse(MANIFEST.as_bytes()).unwrap();
        manifest.identity_name = "Other.Application".into();
        assert!(matches!(manifest.validate(), Err(UwpError::NotMinecraft(_))));

        let mut manifest = PackageManifest::parse(MANIFEST.as_bytes()).unwrap();
        manifest.publisher = "CN=Evil".into();
        assert!(matches!(manifest.validate(), Err(UwpError::NotMinecraft(_))));

        let mut manifest = PackageManifest::parse(MANIFEST.as_bytes()).unwrap();
        manifest.executable = "evil.exe".into();
        assert!(matches!(manifest.validate(), Err(UwpError::NotMinecraft(_))));

        let mut manifest = PackageManifest::parse(MANIFEST.as_bytes()).unwrap();
        manifest.architecture = "arm64".into();
        assert!(matches!(
            manifest.validate(),
            Err(UwpError::UnsupportedArchitecture(_))
        ));

        let mut manifest = PackageManifest::parse(MANIFEST.as_bytes()).unwrap();
        manifest.application_id = "App!x".into();
        assert!(matches!(manifest.validate(), Err(UwpError::Manifest(_))));
    }

    #[test]
    fn preview_identity_uses_preview_family() {
        let manifest = PackageManifest::parse(
            MANIFEST
                .replace("Microsoft.MinecraftUWP", "Microsoft.MinecraftWindowsBeta")
                .as_bytes(),
        )
        .unwrap();
        assert!(manifest.is_preview());
        assert_eq!(
            manifest.family_name(),
            "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe"
        );
        manifest.validate().unwrap();
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        for bad in [
            "../escape",
            "a/../../b",
            "/absolute",
            "C:/windows/system32/x.dll",
            "\\\\server\\share\\x.dll",
            "foo:bar",
            "con.txt",
            "a/b ",
            "a/b.",
            "a//b/",
            "",
            "a/.",
        ] {
            assert!(
                safe_relative_path(bad).is_err(),
                "应拒绝不安全路径: {bad:?}"
            );
        }
        assert_eq!(
            safe_relative_path("Windows/Minecraft.Windows.exe").unwrap(),
            PathBuf::from("Windows").join("Minecraft.Windows.exe")
        );
        assert_eq!(
            safe_relative_path("UAP.Assets\\minecraft\\icons\\MCLogo.png").unwrap(),
            PathBuf::from("UAP.Assets/minecraft/icons/MCLogo.png")
        );
    }

    #[test]
    fn extracts_appx_and_skips_signature() {
        let dir = temp_dir("extract");
        let appx = dir.join("pkg.appx");
        write_min_pe(&dir.join("placeholder.exe"), 0x8664);
        let exe = std::fs::read(dir.join("placeholder.exe")).unwrap();
        make_appx(
            &appx,
            MANIFEST,
            &[("Minecraft.Windows.exe", &exe), ("data/a.json", b"{}")],
        );
        let out = dir.join("out");
        let mut seen = Vec::new();
        let manifest = extract_package(&appx, &out, &mut |done, total, name| {
            seen.push((done, total, name.to_string()));
        })
        .unwrap();
        assert!(manifest.game_version().is_some());
        assert!(out.join("Minecraft.Windows.exe").is_file());
        assert!(out.join("data/a.json").is_file());
        // 签名不解（留着它会让松散注册被系统拒绝）。
        assert!(!out.join(APPX_SIGNATURE).exists());
        assert!(!seen.is_empty(), "解包必须上报进度");
    }

    #[test]
    fn extraction_requires_real_x64_executable() {
        let dir = temp_dir("exe");
        let appx = dir.join("pkg.appx");
        write_min_pe(&dir.join("placeholder.exe"), 0x8664);
        let exe = std::fs::read(dir.join("placeholder.exe")).unwrap();
        make_appx(&appx, MANIFEST, &[("Minecraft.Windows.exe", &exe)]);
        let out = dir.join("out");
        extract_package(&appx, &out, &mut |_, _, _| {}).unwrap();

        // 换成 x86 的 exe：必须拒绝（架构不对的包启动必然失败）。
        let dir = temp_dir("exe32");
        let appx = dir.join("pkg.appx");
        write_min_pe(&dir.join("placeholder.exe"), 0x014c);
        let exe = std::fs::read(dir.join("placeholder.exe")).unwrap();
        make_appx(&appx, MANIFEST, &[("Minecraft.Windows.exe", &exe)]);
        let out = dir.join("out");
        assert!(matches!(
            extract_package(&appx, &out, &mut |_, _, _| {}),
            Err(UwpError::MissingExecutable)
        ));
    }

    #[test]
    fn extraction_rejects_case_collisions() {
        let dir = temp_dir("collision");
        let appx = dir.join("pkg.appx");
        write_min_pe(&dir.join("placeholder.exe"), 0x8664);
        let exe = std::fs::read(dir.join("placeholder.exe")).unwrap();
        make_appx(
            &appx,
            MANIFEST,
            &[("Minecraft.Windows.exe", &exe), ("DATA/A.json", b"a"), ("data/a.json", b"b")],
        );
        let out = dir.join("out");
        let error = extract_package(&appx, &out, &mut |_, _, _| {}).unwrap_err();
        assert!(matches!(error, UwpError::UnsafeEntry(_)), "{error:?}");
    }

    #[test]
    fn bundle_selects_x64_application_package() {
        // 构造 appxbundle：内嵌一个 x64 子包 + 一个资源包（应被拒绝）。
        let dir = temp_dir("bundle");
        let inner = dir.join("inner.appx");
        write_min_pe(&dir.join("placeholder.exe"), 0x8664);
        let exe = std::fs::read(dir.join("placeholder.exe")).unwrap();
        make_appx(&inner, MANIFEST, &[("Minecraft.Windows.exe", &exe)]);
        let inner_bytes = std::fs::read(&inner).unwrap();
        let bundle_manifest = br#"<?xml version="1.0" encoding="utf-8"?>
<Bundle xmlns="http://schemas.microsoft.com/appx/2013/bundle">
  <Packages>
    <Package Type="application" Architecture="x64" FileName="inner.appx" />
  </Packages>
</Bundle>"#;
        let bundle_path = dir.join("pkg.appxbundle");
        let file = std::fs::File::create(&bundle_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        zip.start_file(BUNDLE_MANIFEST, opts).unwrap();
        zip.write_all(bundle_manifest).unwrap();
        zip.start_file("inner.appx", opts).unwrap();
        zip.write_all(&inner_bytes).unwrap();
        zip.finish().unwrap();

        let out = dir.join("out");
        let manifest = extract_package(&bundle_path, &out, &mut |_, _, _| {}).unwrap();
        assert_eq!(manifest.identity_name, "Microsoft.MinecraftUWP");
        assert!(out.join("Minecraft.Windows.exe").is_file());
        // 内嵌包的临时文件必须被清掉，不能留在版本根下。
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".copper-uwp-"))
            .collect();
        assert!(leftovers.is_empty(), "残留内嵌包: {leftovers:?}");
    }
}

