//! 内容落点解析与解包安装。
//!
//! 落点判定**以文件后缀名为准**，CF 远端类别（`content_type`）仅在后缀名
//! 无法区分行为包 / 材质包时兜底。理由：类别是远端元数据，会缺失、错分类、
//! 滞后于文件实际形态；而用户下载到的那个文件的后缀是既成事实。MCBE 只识别
//! **解包后的目录**，所以走 `Install` 的路径必须真正解包落位——把 `.mcpack`
//! 原样丢进加载目录不会被加载，那是最容易让人以为「装好了」却毫无效果的假成功。
//!
//! 三种落点：
//! - [`PlacementKind::Install`]：解包后原子落位到版本的游戏目录；
//! - [`PlacementKind::DownloadOnly`]：落系统「下载」目录，纯下载不安装
//!   （无实例、或后缀名本就不是可安装内容）；
//! - [`PlacementKind::LlpMod`]：安卓 LL 模组，由 `ll_android` 既有链路处理。
//!
//! 解包 / 落位的安全约束：
//! - zip slip：条目路径必须同时通过 `enclosed_name()` 与 [`sanitize_relative`]，
//!   任何绝对路径 / `..` / 盘符一律拒绝安装整个归档；
//! - 原子落位：新内容先落到同卷的 `.<name>.new`，再 `rename` 到位，替换旧内容
//!   走 `.bak` 回滚。同卷 rename 是原子的，因此不存在「装到一半、加载目录里
//!   是个半截包」的中间态。

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::error::KernelError;
use crate::modules::home::content;
use crate::services::paths::ensure_writable_dir;
use crate::state::KernelContext;

use super::model::{TYPE_BEHAVIOR_PACK, TYPE_SHADER, TYPE_TEXTURE_PACK};

/// 加载目录名（与 `home/content.rs` 的枚举口径一致）。
const BEHAVIOR_PACKS: &str = "behavior_packs";
const RESOURCE_PACKS: &str = "resource_packs";
const DEVELOPMENT_BEHAVIOR_PACKS: &str = "development_behavior_packs";
const WORLDS_DIR: &str = "minecraftWorlds";
const SHARED_GAME_DIR: &str = "games/com.mojang";
const MANIFEST_FILE: &str = "manifest.json";

/// 归档的单个内层根目录名（`.mcaddon` 约定）。
const ADDON_BEHAVIOR_DIR: &str = "pack";
const ADDON_RESOURCE_DIR: &str = "resourcepacks";
/// 解包中间目录名（置于 `com.mojang` 下，与目标同卷；点开头使游戏扫描时忽略）。
const EXTRACT_DIR: &str = ".copper-extract";

// ---------------------------------------------------------------- 分类

/// 按后缀名判定的内容种类。
///
/// 刻意不把 `content_type` 编码进来：它是**兜底**信息，只在
/// [`manifest_module_type`] 里参与判定，混进分类枚举会让「谁说了算」
/// 这件事变得不可见。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtKind {
    /// 行为 / 材质包（`.mcpack`）：解包后按 manifest 判行为还是材质。
    Pack,
    /// 插件包（`.mcaddon`）：可能同时含行为层与材质层。
    Addon,
    /// 模板（`.mctemplate`）。
    Template,
    /// 世界（`.mcworld`）。
    World,
    /// 纯 zip：解包后若非包，降级为纯下载。
    Zip,
    /// 安卓 LL 模组归档。
    Llp,
}

/// 按文件名后缀分类（供投递期与安装期共用）。
pub fn classify(filename: &str) -> Option<ExtKind> {
    classify_extension(filename)
}

/// 目录名兜底：文件名去后缀后净化。
pub fn dir_name_hint(filename: &str) -> String {
    fallback_dir_name(filename)
}

fn classify_extension(filename: &str) -> Option<ExtKind> {
    let lower = filename.to_ascii_lowercase();
    let ext = Path::new(&lower).extension().and_then(|e| e.to_str())?;
    Some(match ext {
        "mcpack" => ExtKind::Pack,
        "mcaddon" => ExtKind::Addon,
        "mctemplate" => ExtKind::Template,
        "mcworld" => ExtKind::World,
        "levipack" | "so" => ExtKind::Llp,
        "zip" => ExtKind::Zip,
        _ => return None,
    })
}

/// 单个安装目标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallTargetPath {
    /// 目标加载目录的绝对路径（不含条目名）。
    pub load_dir: PathBuf,
    /// 目标条目目录名。
    pub dir_name: String,
}

impl InstallTargetPath {
    /// 目标条目的完整绝对路径。
    pub fn full_path(&self) -> PathBuf {
        self.load_dir.join(&self.dir_name)
    }
}

/// 解包安装计划：一个归档可能落 1~2 处（`.mcaddon`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlan {
    pub version: String,
    pub targets: Vec<InstallTargetPath>,
    /// 归档落地后的删除策略：解包成功即删暂存。
    pub staging: String,
}

/// 可序列化的内容根视图。
///
/// 之所以要把 `content::ContentRoots` 转一层再序列化：`ContentRoots` 本身
/// 不带 `Serialize`（内核内部类型，不该为一次 IPC 暴露），而落点必须连同
/// 根路径一起下发到前端 `plan` 结果、并冻结进 `target` 列供安装钩子使用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ContentRootsView {
    /// 共享内容根（`.../com.mojang`）。
    pub com_mojang: String,
    /// 用户目录根（`.../Users`），世界包定位用。
    pub users_root: String,
}

impl From<&content::ContentRoots> for ContentRootsView {
    fn from(roots: &content::ContentRoots) -> Self {
        Self {
            com_mojang: roots.com_mojang.to_string_lossy().into_owned(),
            users_root: roots.users_root.to_string_lossy().into_owned(),
        }
    }
}

impl ContentRootsView {
    /// 还原成安装钩子使用的 `ContentRoots`。
    pub fn to_roots(&self) -> content::ContentRoots {
        content::ContentRoots {
            com_mojang: PathBuf::from(&self.com_mojang),
            users_root: PathBuf::from(&self.users_root),
        }
    }
}

/// 落点类型（前端据此决定是否弹确认）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PlacementKind {
    /// 解包安装到指定版本。`roots` 在此冻结，下载完成钩子直接用它落位。
    Install {
        version: String,
        roots: ContentRootsView,
        notice: Option<String>,
    },
    /// 落系统「下载」目录，不安装。
    DownloadOnly { dir: String },
    /// 安卓 LL 模组（由 `ll_android` 处理，本模块不落位）。
    LlpMod { version: String },
}

/// 落点类型 + 前端可直接展示的信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    #[serde(flatten)]
    pub kind: PlacementKind,
    /// 落点目录（`install` 时为暂存路径，安装完成后改写为真实目录）。
    pub dir: String,
    /// 落点类别（`install` / `download_only`），供下载中心展示。
    pub install_kind: &'static str,
}

impl Placement {
    pub fn version(&self) -> Option<&str> {
        match &self.kind {
            PlacementKind::Install { version, .. } | PlacementKind::LlpMod { version } => {
                Some(version.as_str())
            }
            PlacementKind::DownloadOnly { .. } => None,
        }
    }
}

// ---------------------------------------------------------------- 解析

/// 解析最终落点。
///
/// 判定顺序：
/// 1. 后缀名不是已知归档 → 系统「下载」目录（纯下载）；
/// 2. `.levipack` / `.so` → 交给 `ll_android` 链路；
/// 3. 版本根本无可用实例 → 系统「下载」目录（前端须弹确认告知用户）；
/// 4. 有实例 → 版本内安装目标（`Install`）。
///
/// 第 3 步与「实例存在但内容根不可用」是**两回事**：后者是可恢复的临时状态
/// （非隔离实例没启动过游戏），此时**直接报错**而非降级——降级会让用户以为
/// 内容已装进游戏，实际躺在下载目录里。
pub fn resolve(
    kernel: &KernelContext,
    filename: &str,
    version: Option<&str>,
) -> Result<Placement, KernelError> {
    let Some(ext) = classify_extension(filename) else {
        return Ok(download_only(kernel, filename));
    };
    if ext == ExtKind::Llp {
        let version = version
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                KernelError::InvalidArgument("请先在开始页选择要安装的版本".into())
            })?
            .to_string();
        return Ok(Placement {
            kind: PlacementKind::LlpMod { version },
            dir: String::new(),
            install_kind: "install",
        });
    }

    let Some(version) = version.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(download_only(kernel, filename));
    };
    // 无实例（版本根下没有可用版本）→ 纯下载。前端据此弹确认。
    if crate::modules::home::meta::scan_versions(&kernel.versions_root()).is_empty() {
        return Ok(download_only(kernel, filename));
    }
    // 实例存在但内容根不可用 → 报错，不静默降级。
    let roots = content::content_roots(kernel, version)?;

    // 暂存目录先建好并探测可写：内容根可用但缓存不可写是可能的，
    // 让它在投递前就带着路径报错，而不是等下载完成才失败。
    let staging = staging_path(kernel, filename);
    ensure_writable_dir(&staging_parent(&staging), "内容暂存")?;

    Ok(Placement {
        kind: PlacementKind::Install {
            version: version.to_string(),
            roots: ContentRootsView::from(&roots),
            notice: None,
        },
        dir: staging.to_string_lossy().into_owned(),
        install_kind: "install",
    })
}

/// 落系统「下载」目录。
///
/// 平台不提供下载目录时（Android / 未实现的平台）退回内核缓存目录，并
/// 带上说明——不静默假装「下载目录」可用，那会让提示文案指向一个错的地方。
fn download_only(kernel: &KernelContext, filename: &str) -> Placement {
    let name = sanitize_filename(filename);
    let dirs = kernel.backends().user_dirs().clone();
    match dirs.downloads_dir() {
        Some(dir) => {
            let dest = dir.join(&name);
            if let Err(e) = crate::services::paths::ensure_writable_dir(&dir, "下载目录") {
                log::warn!("[content-download] 下载目录不可写 {dir:?}: {e}");
                return cache_download_only(kernel, filename);
            }
            Placement {
                kind: PlacementKind::DownloadOnly {
                    dir: dir.to_string_lossy().into_owned(),
                },
                dir: dest.to_string_lossy().into_owned(),
                install_kind: "download_only",
            }
        }
        None => cache_download_only(kernel, filename),
    }
}

/// 平台无下载目录时的兜底：内核缓存。
fn cache_download_only(kernel: &KernelContext, filename: &str) -> Placement {
    let dir = kernel.paths().cache_dir().join("content");
    let _ = std::fs::create_dir_all(&dir);
    Placement {
        kind: PlacementKind::DownloadOnly {
            dir: dir.to_string_lossy().into_owned(),
        },
        dir: dir.join(sanitize_filename(filename)).to_string_lossy().into_owned(),
        install_kind: "download_only",
    }
}

/// 暂存归档路径：`cache/content/staging/<净化文件名>`。
pub fn staging_path(kernel: &KernelContext, filename: &str) -> PathBuf {
    let dir = kernel.paths().cache_dir().join("content").join("staging");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(sanitize_filename(filename))
}

fn staging_parent(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

// ---------------------------------------------------------------- 解包安装

/// 暂存归档解包并原子落位。
///
/// 返回落位目录（`.mcaddon` 双落位时返回主目标，即行为层目录）。
pub fn install_staged(
    archive: &Path,
    ext: ExtKind,
    content_type: &str,
    roots: &content::ContentRoots,
    dir_name_hint: &str,
) -> Result<PathBuf, KernelError> {
    if !archive.is_file() {
        return Err(KernelError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("内容归档不存在：{}", archive.display()),
        )));
    }
    // 解包必须与目标加载目录**同卷**：落位靠 `rename`（同卷才原子，跨卷在
    // Windows 直接失败、在 Unix 退化成隐式拷贝）。内容根与内核缓存目录
    // `cache` 通常不在同一分区，因此不能解到 `archive` 旁边。
    //
    // 放在 `com.mojang` 下而不是加载目录里：`.copper-extract` 落在
    // `behavior_packs/` 内会被游戏当成一个坏掉的包扫到。
    let extract_root = roots.com_mojang.join(EXTRACT_DIR);
    let _ = std::fs::remove_dir_all(&extract_root);
    ensure_writable_dir(&extract_root, "内容解包")?;
    let outcome = extract_archive(archive, &extract_root)
        .and_then(|extracted| dispatch(extracted, ext, content_type, roots, dir_name_hint));
    // 解包中间态无价值，无论成败都清掉。
    let _ = std::fs::remove_dir_all(&extract_root);
    let placed = outcome?;

    // 归档已成功落位，暂存可以删了（失败时保留便于重试与排查）。
    let _ = std::fs::remove_file(archive);
    Ok(placed)
}

/// 按后缀名与解出结构决定落点并落位。
fn dispatch(
    extracted: PathBuf,
    ext: ExtKind,
    content_type: &str,
    roots: &content::ContentRoots,
    dir_name_hint: &str,
) -> Result<PathBuf, KernelError> {
    match ext {
        ExtKind::World => {
            let dir_name = read_pack_name(&extracted).unwrap_or_else(|| dir_name_hint.to_string());
            let target = InstallTargetPath {
                load_dir: worlds_dir_for(roots),
                dir_name,
            };
            place_one(&extracted, &target)
        }
        ExtKind::Template => {
            let dir_name = read_pack_name(&extracted).unwrap_or_else(|| dir_name_hint.to_string());
            let target = InstallTargetPath {
                load_dir: roots.com_mojang.join(DEVELOPMENT_BEHAVIOR_PACKS),
                dir_name,
            };
            place_one(&extracted, &target)
        }
        ExtKind::Pack | ExtKind::Zip => {
            let root = resolve_pack_root(&extracted).ok_or_else(|| {
                KernelError::InvalidArgument(
                    "压缩包内未找到有效的资源包（缺少 manifest.json）".into(),
                )
            })?;
            let load_dir = match manifest_module_type(&root, content_type) {
                Some(ModuleType::Data) => roots.com_mojang.join(RESOURCE_PACKS),
                _ => roots.com_mojang.join(BEHAVIOR_PACKS),
            };
            let dir_name = read_pack_name(&root).unwrap_or_else(|| dir_name_hint.to_string());
            let target = InstallTargetPath {
                load_dir,
                dir_name,
            };
            place_one(&root, &target)
        }
        ExtKind::Addon => place_addon(&extracted, roots, dir_name_hint),
        ExtKind::Llp => Err(KernelError::InvalidArgument(
            "安卓模组归档不由本路径安装".into(),
        )),
    }
}

/// `.mcaddon`：可能同时含行为层与材质层，分别落位。
///
/// 官方语义下一个 addon 就是「一个行为包 + 一个材质包」，只落其中一半会让用户
/// 在游戏里看到能进世界却没材质（或反之）的半吊子状态。
fn place_addon(
    extracted: &Path,
    roots: &content::ContentRoots,
    dir_name_hint: &str,
) -> Result<PathBuf, KernelError> {
    let mut placed = Vec::new();
    let behavior = extracted.join(ADDON_BEHAVIOR_DIR);
    let resources = extracted.join(ADDON_RESOURCE_DIR);

    if behavior.join(MANIFEST_FILE).is_file() {
        let dir_name = read_pack_name(&behavior).unwrap_or_else(|| dir_name_hint.to_string());
        placed.push(place_one(
            &behavior,
            &InstallTargetPath {
                load_dir: roots.com_mojang.join(BEHAVIOR_PACKS),
                dir_name,
            },
        )?);
    }
    // 行为层落位失败即中止：此时还没有任何东西落位，无需回滚。
    if resources.join(MANIFEST_FILE).is_file() {
        let dir_name = read_pack_name(&resources).unwrap_or_else(|| dir_name_hint.to_string());
        match place_one(
            &resources,
            &InstallTargetPath {
                load_dir: roots.com_mojang.join(RESOURCE_PACKS),
                dir_name,
            },
        ) {
            Ok(path) => placed.push(path),
            // 材质层失败 → 撤掉刚落的行为层。只装一半比不装更糟：
            // 用户在游戏里会看到能进世界却没有材质（或反之）的半吊子状态，
            // 还会以为是自己哪里操作错了。
            Err(e) => {
                for path in placed.iter() {
                    let _ = std::fs::remove_dir_all(path);
                }
                return Err(e);
            }
        }
    }
    if placed.is_empty() {
        // addon 布局对不上：宁可不装，也不猜一个目录硬塞。
        return Err(KernelError::InvalidArgument(
            "插件包内未找到 pack/ 或 resourcepacks/ 目录，无法确定安装位置".into(),
        ));
    }
    Ok(placed[0].clone())
}

/// 把 `src` 目录内容原子搬到 `target`。
///
/// 流程：目标已存在 → 改名为 `.<name>.bak`；`src` 改名为目标；成功后删 `.bak`。
/// 中途失败回滚 `.bak`，绝不留下「目录不存在」的窗口——加载目录里条目
/// 忽有忽无会让游戏侧的行为不可预测。
fn place_one(src: &Path, target: &InstallTargetPath) -> Result<PathBuf, KernelError> {
    let load_dir = target.load_dir.as_path();
    let final_path = target.full_path();
    std::fs::create_dir_all(load_dir)?;

    let staging = load_dir.join(format!(".{}.new", target.dir_name));
    let backup = load_dir.join(format!(".{}.bak", target.dir_name));
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&backup);
    copy_dir(src, &staging)?;

    let had_previous = final_path.exists();
    if had_previous {
        std::fs::rename(&final_path, &backup).map_err(|e| {
            KernelError::Io(std::io::Error::new(
                e.kind(),
                format!("备份已存在内容失败（{}）：{e}", final_path.display()),
            ))
        })?;
    }
    if let Err(e) = std::fs::rename(&staging, &final_path) {
        if had_previous {
            let _ = std::fs::rename(&backup, &final_path);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(KernelError::Io(std::io::Error::new(
            e.kind(),
            format!("落位失败（{}）：{e}", final_path.display()),
        )));
    }
    if had_previous {
        let _ = std::fs::remove_dir_all(&backup);
    }
    Ok(final_path)
}

/// 世界包落点目录。
///
/// 与 `home/content.rs::first_player_worlds_dir` 同一口径：真实存档在
/// `<Users>/<首个玩家>/games/com.mojang/minecraftWorlds`，找不到玩家目录时
/// 退回 `<com.mojang>/minecraftWorlds`。这里重写一遍而不是调用内核的
/// `content::worlds_dir`，是因为下载完成钩子拿不到 `KernelContext`（落点
/// 已在投递时冻结），而扫描 `users_root` 本身只需要冻结下来的两个路径。
fn worlds_dir_for(roots: &content::ContentRoots) -> PathBuf {
    let shared = roots.com_mojang.join(SHARED_GAME_DIR).join(WORLDS_DIR);
    let Ok(entries) = std::fs::read_dir(&roots.users_root) else {
        return shared;
    };
    let mut fallback: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case("Shared") || name.starts_with('.') {
            continue;
        }
        let worlds = path.join(SHARED_GAME_DIR).join(WORLDS_DIR);
        if worlds.is_dir() {
            return worlds;
        }
        fallback.get_or_insert(path);
    }
    fallback
        .map(|p| p.join(SHARED_GAME_DIR).join(WORLDS_DIR))
        .unwrap_or(shared)
}

/// 递归复制目录（不跟随符号链接——归档里出现链接即拒绝，见 `extract_archive`）。
fn copy_dir(src: &Path, dst: &Path) -> Result<(), KernelError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| {
                KernelError::Io(std::io::Error::new(
                    e.kind(),
                    format!("复制 {} 失败：{e}", from.display()),
                ))
            })?;
        }
    }
    Ok(())
}

/// 解包 zip 到 `out_dir`。
///
/// 双层防护：先 `enclosed_name()` 拒绝穿越路径，再 [`sanitize_relative`] 拒绝
/// 绝对路径 / 盘符 / `..`。单层防护不够——`enclosed_name` 只规范化分隔符，
/// `C:/x` 这类带盘符的条目在 Windows 上仍能拼出越界路径。
fn extract_archive(archive: &Path, out_dir: &Path) -> Result<PathBuf, KernelError> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| {
        KernelError::InvalidArgument(format!("内容归档不是有效的 zip：{e}"))
    })?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| {
            KernelError::InvalidArgument(format!("读取归档条目失败：{e}"))
        })?;
        let Some(raw_name) = entry.enclosed_name() else {
            return Err(KernelError::InvalidArgument(
                "归档含越界路径条目，已拒绝安装".into(),
            ));
        };
        if entry.is_dir() {
            continue;
        }
        // 符号链接在 Windows 上不常用，但 Unix 打包的包可能带；
        // 其目标路径同样可能越界，直接拒绝而不是按普通文件写出去。
        if entry
            .unix_mode()
            .is_some_and(|m| m & 0o170_000 == 0o120_000)
        {
            return Err(KernelError::InvalidArgument(
                "归档含符号链接条目，已拒绝安装".into(),
            ));
        }
        let rel = raw_name.to_string_lossy().replace('\\', "/");
        let Some(safe) = sanitize_relative(&rel) else {
            return Err(KernelError::InvalidArgument(format!(
                "归档含非法路径条目 `{rel}`，已拒绝安装"
            )));
        };
        let target = out_dir.join(&safe);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut output)?;
    }
    Ok(out_dir.to_path_buf())
}

/// 规范化归档内相对路径：`..` / 空段 / 盘符 / 绝对路径一律拒绝。
fn sanitize_relative(rel: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in rel.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            p if p.contains(':') => return None,
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// 定位包根：manifest 在解包根上，或在一层子目录里（打包时多套了一层）。
fn resolve_pack_root(extracted: &Path) -> Option<PathBuf> {
    if extracted.join(MANIFEST_FILE).is_file() {
        return Some(extracted.to_path_buf());
    }
    let entries = std::fs::read_dir(extracted).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join(MANIFEST_FILE).is_file() {
            return Some(path);
        }
    }
    None
}

/// 读 `manifest.json` 的 `pack_name`（MCBE 显示名）。
fn read_pack_name(root: &Path) -> Option<String> {
    let raw = std::fs::read(root.join(MANIFEST_FILE)).ok()?;
    let manifest: Value = serde_json::from_slice(&raw).ok()?;
    let name = manifest.get("pack_name")?.as_str()?.trim();
    sanitize_dir_name(name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleType {
    /// 材质 / 光影（`resources`）。
    Resources,
    /// 行为（`data`）。
    Data,
}

/// 判定行为包还是材质包。
///
/// manifest 的 `modules[].type` 是权威（`data` = 行为，`resources` = 材质）；
/// 缺失时用 CF 类别兜底（`behavior_pack` → 行为，其余 → 材质）。这是唯一
/// 允许 `content_type` 参与判定的地方。
fn manifest_module_type(root: &Path, content_type: &str) -> Option<ModuleType> {
    let raw = std::fs::read(root.join(MANIFEST_FILE)).ok()?;
    let manifest: Value = serde_json::from_slice(&raw).ok()?;
    let modules = manifest.get("modules")?.as_array()?;
    let declared = modules
        .iter()
        .filter_map(|m| m.get("type").and_then(Value::as_str))
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if declared.iter().any(|t| t == "data") {
        return Some(ModuleType::Data);
    }
    if declared.iter().any(|t| t == "resources" || t == "shader") {
        return Some(ModuleType::Resources);
    }
    Some(fallback_module_type(content_type))
}

/// CF 类别兜底。
fn fallback_module_type(content_type: &str) -> ModuleType {
    match content_type {
        TYPE_BEHAVIOR_PACK => ModuleType::Data,
        TYPE_TEXTURE_PACK | TYPE_SHADER => ModuleType::Resources,
        // 未知类别按材质处理：材质包放错目录只是不生效，行为包放错目录
        // 可能影响世界加载，保守的一侧是材质。
        _ => ModuleType::Resources,
    }
}

// ---------------------------------------------------------------- 净化

/// 净化目录名（防路径穿越 / 保留名 / 控制字符）。
///
/// 比 [`sanitize_filename`] 更严：目录名会参与 `place_one` 的
/// `.<name>.new` / `.<name>.bak` 临时名，必须排除 `.`、`..` 这类整名。
pub fn sanitize_dir_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// 解包前用不上的目录名兜底：文件名去后缀。
fn fallback_dir_name(filename: &str) -> String {
    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "content".to_string());
    sanitize_dir_name(&stem).unwrap_or_else(|| "content".to_string())
}

/// 净化写入磁盘的文件名（防路径穿越）。
pub fn sanitize_filename(name: &str) -> String {
    let base = name.trim();
    if base.is_empty() {
        return "download.bin".to_string();
    }
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "download.bin".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_classification_covers_known_suffixes() {
        assert_eq!(classify_extension("A.mcpack"), Some(ExtKind::Pack));
        assert_eq!(classify_extension("a.MCPACK"), Some(ExtKind::Pack));
        assert_eq!(classify_extension("a.mcaddon"), Some(ExtKind::Addon));
        assert_eq!(classify_extension("a.mctemplate"), Some(ExtKind::Template));
        assert_eq!(classify_extension("a.mcworld"), Some(ExtKind::World));
        assert_eq!(classify_extension("a.zip"), Some(ExtKind::Zip));
        assert_eq!(classify_extension("a.levipack"), Some(ExtKind::Llp));
        assert_eq!(classify_extension("a.dll"), None);
        assert_eq!(classify_extension("无后缀"), None);
    }

    /// zip slip：穿越 / 盘符 / 绝对路径必须被拒。
    #[test]
    fn sanitize_relative_rejects_escapes() {
        assert_eq!(sanitize_relative("pack/manifest.json").as_deref(), Some("pack/manifest.json"));
        assert_eq!(sanitize_relative("./pack/a.json").as_deref(), Some("pack/a.json"));
        assert_eq!(sanitize_relative("../evil"), None);
        assert_eq!(sanitize_relative("a/../../evil"), None);
        assert_eq!(sanitize_relative("C:/Windows/evil"), None);
        assert_eq!(sanitize_relative(""), None);
    }

    #[test]
    fn dir_name_sanitization_drops_traversal() {
        assert_eq!(sanitize_dir_name("  ..  "), None);
        assert_eq!(sanitize_dir_name("."), None);
        assert_eq!(sanitize_dir_name("a/b").as_deref(), Some("a_b"));
        assert_eq!(sanitize_dir_name("正常名").as_deref(), Some("正常名"));
    }

    #[test]
    fn fallback_module_type_by_content_type() {
        assert_eq!(fallback_module_type(TYPE_BEHAVIOR_PACK), ModuleType::Data);
        assert_eq!(fallback_module_type(TYPE_TEXTURE_PACK), ModuleType::Resources);
        assert_eq!(fallback_module_type(TYPE_SHADER), ModuleType::Resources);
        assert_eq!(fallback_module_type("未知"), ModuleType::Resources);
    }

    /// 解包后单层 / 多套一层的包都能定位到 manifest。
    #[test]
    fn pack_root_resolves_flat_and_nested() {
        let base = std::env::temp_dir().join("copper-pack-root-test");
        let _ = std::fs::remove_dir_all(&base);
        let flat = base.join("flat");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::write(flat.join(MANIFEST_FILE), "{}").unwrap();
        assert_eq!(resolve_pack_root(&flat), Some(flat.clone()));

        let nested = base.join("nested");
        std::fs::create_dir_all(nested.join("inner")).unwrap();
        std::fs::write(nested.join("inner").join(MANIFEST_FILE), "{}").unwrap();
        assert_eq!(resolve_pack_root(&nested), Some(nested.join("inner")));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// manifest 的 modules.type 优先于 CF 类别。
    #[test]
    fn manifest_module_type_wins_over_content_type() {
        let base = std::env::temp_dir().join("copper-manifest-type-test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join(MANIFEST_FILE),
            r#"{"pack_name":"x","modules":[{"type":"data"}]}"#,
        )
        .unwrap();
        // CF 类别说材质包，但 manifest 说行为包 → 听 manifest 的。
        assert_eq!(manifest_module_type(&base, TYPE_TEXTURE_PACK), Some(ModuleType::Data));

        std::fs::write(
            base.join(MANIFEST_FILE),
            r#"{"pack_name":"x","modules":[{"type":"resources"}]}"#,
        )
        .unwrap();
        assert_eq!(
            manifest_module_type(&base, TYPE_BEHAVIOR_PACK),
            Some(ModuleType::Resources)
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// place_one 必须真正落位，且不留 `.new` / `.bak` 残留。
    #[test]
    fn place_one_lands_and_cleans_up() {
        let base = std::env::temp_dir().join("copper-place-one-test");
        let _ = std::fs::remove_dir_all(&base);
        let load = base.join("behavior_packs");
        let src = base.join("src");
        std::fs::create_dir_all(&load).unwrap();
        std::fs::create_dir_all(src.join("textures")).unwrap();
        std::fs::write(src.join(MANIFEST_FILE), "{}").unwrap();
        std::fs::write(src.join("textures").join("a.png"), "x").unwrap();

        let target = InstallTargetPath {
            load_dir: load.clone(),
            dir_name: "pack".into(),
        };
        let landed = place_one(&src, &target).unwrap();
        assert_eq!(landed, load.join("pack"));
        assert!(landed.join("textures").join("a.png").is_file());

        // 二次安装覆盖同名内容。
        std::fs::write(src.join(MANIFEST_FILE), r#"{"v":2}"#).unwrap();
        place_one(&src, &target).unwrap();
        assert!(landed.join(MANIFEST_FILE).is_file());

        let leftovers: Vec<String> = std::fs::read_dir(&load)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "残留临时目录: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
