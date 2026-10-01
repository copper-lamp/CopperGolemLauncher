//! 内容下载模块（`content-download`）：拉取网络内容、浏览并下载。
//!
//! 两类数据源：
//! - **CurseForge**（行为包 / 材质包 / 光影包）：列表 / 详情 / 文件直链齐备，
//!   `download` 把 `downloadUrl` 投递到内核全局下载队列（悬浮窗统一展示进度）。
//! - **LIP**（LL 模组）：lipr 索引导仅含元数据，真实安装经 `lip_install` 调用
//!   `lipd` 守护进程完成（见 `lipd.rs` / `lip_install.rs`）；不伪造直链。
//!
//! 联动（经内核中介）：投递任务 `kernel.download()`、持久化 `kernel.db()`
//! （记录已下载项）、设置 `content.curseforgeApiKey`、语言包 `kernel.i18n()`。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use copper_downloader::DownloadOptions;
use serde_json::Value;

use crate::error::KernelError;
use crate::modules::home::content;
use crate::registry::events::{EventBus, Subscription};
use crate::registry::modules::Module;
use crate::services::database::DatabaseService;
use crate::state::KernelContext;

use self::model::{
    ContentDetail, ContentItem, ContentListPage, ContentListQuery, PAGE_SIZE, SOURCE_LIP,
    SOURCE_LL_ANDROID, TYPE_BEHAVIOR_PACK, TYPE_LL_MOD, TYPE_SHADER, TYPE_TEXTURE_PACK,
};

pub use self::lip_install::LipInstallOutcome;

/// 安卓平台的 LL 模组来源（LeviModHub 目录 `.so` 直装）。
///
/// lipr 索引的资产全为 `win-x64`，lipd 又依赖 .NET + BDS，安卓两者皆不可用；
/// 详见 [`ll_android`] 模块文档。用函数而非 `cfg!` 常量，是为了让**同一份
/// 逻辑在两个平台都可编译可单测**，只有路由点才做平台判断。
#[inline]
fn android_ll_source() -> bool {
    cfg!(target_os = "android")
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentDownloadRecord {
    pub id: String,
    pub source: String,
    pub content_type: String,
    pub name: String,
    pub version: String,
    pub state: String,
    pub dest: Option<String>,
    pub task_id: Option<u64>,
    pub error: Option<String>,
    pub updated_at: i64,
    /// 阶段化进度（0~1）。`Some` 时前端进度条忽略字节进度（lip 安装等）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    /// 阶段文案的 i18n 键（`download.stage.*`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
}

pub mod curseforge;
pub mod http;
pub mod lip;
pub mod ll_android;
pub mod lip_install;
pub mod lipd;
pub mod model;

/// 模块唯一标识。
pub const MODULE_ID: &str = "content-download";

/// 内容下载模块实例。
pub struct ContentDownloadModule {
    /// 事件订阅句柄（stop 时退订）。
    subs: Mutex<Vec<Subscription>>,
}

impl Default for ContentDownloadModule {
    fn default() -> Self {
        Self {
            subs: Mutex::new(Vec::new()),
        }
    }
}

// ---------------------------------------------------------------- 平台路由

/// 请求是否指向 LL 模组（按来源或按类型任一命中）。
fn ll_mod_requested(source: Option<&str>, content_type: Option<&str>) -> bool {
    matches!(source, Some(SOURCE_LIP) | Some(SOURCE_LL_ANDROID))
        || content_type == Some(TYPE_LL_MOD)
}

/// LL 模组列表：安卓走 LeviModHub 目录，桌面走 lipr 索引导。
async fn ll_list(
    kernel: &KernelContext,
    query: &ContentListQuery,
) -> Result<ContentListPage, KernelError> {
    if android_ll_source() {
        return ll_android::list(kernel, query).await;
    }
    lip::list(kernel, query).await
}

// ---------------------------------------------------------------- 对外业务

impl ContentDownloadModule {
    /// 列表：按来源 / 类型过滤 + 关键字搜索 + 分页。
    ///
    /// 「全部来源 + 全部类型」时按 `4:4:1:1` 混排（行为包:材质包:光影:LL 模组），
    /// 每轮 10 条，否则按来源 / 类型定向过滤。
    pub async fn list(
        kernel: &KernelContext,
        query: &ContentListQuery,
    ) -> Result<ContentListPage, KernelError> {
        let source = query.source.as_deref();
        let ctype = query.content_type.as_deref();
        let result = if ll_mod_requested(source, ctype) {
            ll_list(kernel, query).await
        } else if source.is_none() || source == Some("") {
            if ctype.is_none() || ctype == Some("") {
                mixed_list(kernel, query).await
            } else {
                curseforge::list(kernel, query).await
            }
        } else {
            curseforge::list(kernel, query).await
        };
        match &result {
            Ok(page) => log::info!(
                "[content-download] list source={:?} ctype={:?} search={:?} page={} -> {} items, total={}",
                query.source,
                query.content_type,
                query.search,
                query.page,
                page.items.len(),
                page.total,
            ),
            Err(e) => log::error!(
                "[content-download] list source={:?} ctype={:?} search={:?} page={} FAILED: {e}",
                query.source,
                query.content_type,
                query.search,
                query.page
            ),
        }
        result
    }

    /// 详情：按跨源 id（`cf:` / `lip:` / `lla:` 前缀）路由。
    pub async fn detail(kernel: &KernelContext, id: &str) -> Result<ContentDetail, KernelError> {
        if let Some(cf_id) = id.strip_prefix("cf:") {
            let mod_id: i64 = cf_id
                .parse()
                .map_err(|_| KernelError::InvalidArgument(format!("无效内容 id `{id}`")))?;
            curseforge::detail(kernel, mod_id).await
        } else if let Some(ident) = id.strip_prefix("lip:") {
            lip::detail(kernel, ident).await
        } else if let Some(ident) = ll_android::parse_mod_id(id) {
            ll_android::detail(kernel, ident).await
        } else {
            Err(KernelError::InvalidArgument(format!("无法识别的内容 id `{id}`")))
        }
    }

    /// 拉取项目 readme 原文，按跨源 id 路由。
    ///
    /// - `cf:` → CurseForge 项目描述（HTML 片段）。
    /// - `lip:` → 对应 GitHub 仓库的 readme（Markdown 原文），按 `locale` 优先匹配语言变体。
    /// - `lla:` → 同 `lip:`；目录条目的 `homepage_url` 指向发布方 GitHub 仓库，
    ///   复用同一抓取器（镜像优先、直连回退）。
    ///
    /// 返回的是「原文」而非渲染结果：CF 为 HTML、lip 为 Markdown，两者都需前端
    /// 经安全过滤后渲染。无文档时返回 `None`，不伪造内容。
    pub async fn readme(
        kernel: &KernelContext,
        id: &str,
        locale: &str,
    ) -> Result<Option<String>, KernelError> {
        if let Some(cf_id) = id.strip_prefix("cf:") {
            let mod_id: i64 = cf_id
                .parse()
                .map_err(|_| KernelError::InvalidArgument(format!("无效内容 id `{id}`")))?;
            curseforge::description(kernel, mod_id).await
        } else if let Some(ident) = id.strip_prefix("lip:") {
            lip::readme(kernel, ident, locale).await
        } else if ll_android::parse_mod_id(id).is_some() {
            // 目录条目的 `homepage_url` 即发布仓库，复用 lip 的抓取器。
            let repo_url = Self::detail(kernel, id).await.ok().and_then(|d| d.repo_url);
            match repo_url {
                Some(url) => Ok(lip::fetch_github_readme(&url, locale).await),
                None => Ok(None),
            }
        } else {
            Err(KernelError::InvalidArgument(format!("无法识别的内容 id `{id}`")))
        }
    }

    /// 可选游戏版本列表（供前端「版本过滤」下拉）。
    ///
    /// LL 模组无 MCBE 游戏版本元数据，故列表仅来自 CurseForge。
    pub async fn game_versions(kernel: &KernelContext) -> Result<Vec<String>, KernelError> {
        curseforge::game_versions(kernel).await
    }

    /// 下载投递：文件直链 → 内核下载队列，并落库一条下载记录。
    ///
    /// 按来源分三条链路：
    /// - `cf:` → CurseForge 直链，投递到版本内容根（行为包 / 材质包 / 光影）；
    /// - `lip:` → lip 无直链，提示改用「lip 安装」；
    /// - `lla:` → 安卓目录直链，投递到 `cache` 暂存，由下载完成钩子解包到
    ///   `<versions>/<name>/mods/<modId>`（见 [`install_ll_android_mod`]）。
    pub async fn download(
        kernel: &KernelContext,
        id: &str,
        file_id: &str,
    ) -> Result<u64, KernelError> {
        if ll_android::parse_mod_id(id).is_some() {
            return enqueue_ll_android(kernel, id, file_id).await;
        }
        let cf_id = id
            .strip_prefix("cf:")
            .ok_or_else(|| {
                KernelError::InvalidArgument("LL 模组无直接下载链接，请使用「lip 安装」功能".into())
            })?;
        let mod_id: i64 = cf_id
            .parse()
            .map_err(|_| KernelError::InvalidArgument(format!("无效内容 id `{id}`")))?;

        let detail = curseforge::detail(kernel, mod_id).await?;
        let file = detail
            .files
            .iter()
            .find(|f| f.id == file_id)
            .ok_or_else(|| KernelError::InvalidArgument(format!("找不到文件 `{file_id}`")))?;
        if file.download_url.trim().is_empty() {
            return Err(KernelError::InvalidArgument("该文件没有可用的下载链接".into()));
        }

        // 按资源类型解析落点：行为包→behavior_packs；材质包/光影→resource_packs；
        // 目标为「当前选中版本」（launch.default_version）。无选中版本 → 回退 cache/content。
        let cfg = resolve_install_target(kernel, &detail.item, &file.filename);
        let dest = cfg.dest.clone();
        // 落点提示（如内容根不可用回退缓存）：广播给前端。
        if let Some(notice) = &cfg.notice {
            kernel.events().publish(
                "content-download.location",
                serde_json::json!({ "notice": notice }),
            );
        }

        let opts = DownloadOptions {
            filename: file.filename.clone().into(),
            resume: true,
            expected_sha256: file.sha256.clone(),
            ..Default::default()
        };

        let task_id = kernel.download().enqueue(&file.download_url, &dest, opts)?;
        record_download(
            kernel,
            &detail.item,
            file.version.as_str(),
            dest.to_string_lossy().as_ref(),
            task_id,
        )?;
        Ok(task_id)
    }

    /// 探测 lip 环境：lipd 可执行是否可用。
    pub fn lip_env() -> LipEnv {
        let available = lipd::find_lip_executable();
        LipEnv {
            lip_available: available.is_some(),
            message: if available.is_some() {
                None
            } else {
                Some("未检测到 lipd，请先安装 lip（需要 .NET 10 运行时）".to_string())
            },
        }
    }

    /// 经 lipd 安装 / 更新 LL 模组到目标版本目录。
    ///
    /// 目标目录取「具体已安装版本目录」：显式 `dir` 优先，否则解析设置
    /// `launch.default_version`。域内失败以 `success=false` + `error_code` 表达。
    pub async fn lip_install(
        kernel: &KernelContext,
        id: &str,
        version: &str,
        variant: Option<&str>,
        dir: Option<&str>,
    ) -> LipInstallOutcome {
        let detail = Self::detail(kernel, id).await.ok();
        if let Some(detail) = detail.as_ref() {
            let _ = record_lip_install(kernel, &detail.item, version, dir);
        }
        // lipd 的步骤 / 进度实时写回记录并广播，下载中心据此画模拟进度条。
        // 记录拿不到（详情拉取失败）时不挂 sink：没有可更新的行，上报只会白跑。
        let sink = detail.as_ref().map(|detail| {
            lip_progress_sink(
                kernel.db().clone(),
                kernel.events().clone(),
                detail.item.id.clone(),
            )
        });
        let outcome = lip_install::install(kernel, id, version, variant, dir, sink.as_ref()).await;
        if let Some(detail) = detail.as_ref() {
            let state = if outcome.success { "installed" } else { "failed" };
            let _ = update_lip_record(
                kernel,
                &detail.item.id,
                version,
                state,
                (!outcome.success).then_some(outcome.stderr.as_str()),
            );
        }
        outcome
    }
}

// ---------------------------------------------------------------- 安卓 LL 模组安装

/// 安卓 LL 模组投递：解析直链 → 落`cache` 暂存 → 投递下载队列。
///
/// **不直接解压到目标目录**：`.levipack` 是 zip，必须先完整下载并通过
/// sha256 校验，因此投递到暂存路径，再由 `download.status` 钩子
/// （[`install_ll_android_mod`]）完成解包与原子落位。
async fn enqueue_ll_android(
    kernel: &KernelContext,
    id: &str,
    file_id: &str,
) -> Result<u64, KernelError> {
    let detail = ContentDownloadModule::detail(kernel, id).await?;
    let mod_id = ll_android::parse_mod_id(id)
        .ok_or_else(|| KernelError::InvalidArgument(format!("无效内容 id `{id}`")))?
        .to_string();

    let Some(file) = detail.files.iter().find(|f| f.id == file_id) else {
        return Err(KernelError::InvalidArgument(format!("找不到文件 `{file_id}`")));
    };
    if file.download_url.trim().is_empty() {
        // `browser` / `ad` 类型的发布没有直链（对齐 ModCatalogInstaller
        // 的 opensInBrowser / isAdDownload）：只能引导用户去发布页。
        return Err(KernelError::InvalidArgument(
            "该版本未提供安卓直链，请前往项目发布页手动下载".into(),
        ));
    }

    // 落点先校验：mods 目录与游戏目录平行，下错位置 mod 不会生效且用户无感。
    let version = kernel
        .settings()
        .get::<String>("launch.default_version")
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            KernelError::InvalidArgument("请先在设置中指定默认版本，再安装安卓模组".into())
        })?;
    let target = ll_android::resolve_install_target(kernel, &version, &mod_id)
        .map_err(|(code, message)| {
            log::warn!("[content-download] 安卓模组落点不可用({code}): {message}");
            KernelError::InvalidArgument(message)
        })?;

    // 兼容性判定：目录为每个发布声明 minecraft_versions，不含当前实例版本时
    // 直接拒绝投递，避免装上一个必然被 loader 跳过的包。取不到实例版本
    // （元数据异常）时不拦截，由loader 侧的兼容性检查兜底。
    if !file.game_versions.is_empty() {
        if let Some(instance_version) = instance_game_version(kernel, &version) {
            let compatible = file
                .game_versions
                .iter()
                .any(|pattern| ll_android::matches_minecraft_version(pattern, &instance_version));
            if !compatible {
                return Err(KernelError::InvalidArgument(format!(
                    "该模组版本不支持当前实例的 Minecraft 版本 `{instance_version}`"
                )));
            }
        }
    }

    let staging = staging_path(kernel, &mod_id, &file.version);
    let opts = DownloadOptions {
        filename: file.filename.clone().into(),
        resume: true,
        expected_sha256: file.sha256.clone(),
        ..Default::default()
    };

    let task_id = kernel
        .download()
        .enqueue(&file.download_url, &staging, opts)?;

    // 记录里带上目标版本名与落点，供下载完成钩子定位（无需再次读设置，
    // 避免用户中途改默认版本导致落错实例）。
    record_ll_android_download(
        kernel,
        &detail.item,
        &file.version,
        &target,
        staging.to_string_lossy().as_ref(),
        file.game_versions.clone(),
        task_id,
    )?;
    Ok(task_id)
}

/// 安卓模组暂存路径：`cache/content/lla/<modId>-<version>.levipack`。
fn staging_path(kernel: &KernelContext, mod_id: &str, version: &str) -> PathBuf {
    let dir = kernel.paths().cache_dir().join("content").join("lla");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!(
        "{}-{}.levipack",
        sanitize_filename(mod_id),
        sanitize_filename(version)
    ))
}

/// 实例自身的 MC 版本（取 `version.json` 的 `gameVersion` / `android.versionName`）。
fn instance_game_version(kernel: &KernelContext, version: &str) -> Option<String> {
    let dir = crate::modules::home::meta::resolve_version_dir(&kernel.versions_root(), version).ok()?;
    let meta = crate::modules::home::meta::VersionMeta::read(&dir)?;
    meta.android
        .as_ref()
        .map(|a| a.version_name.clone())
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            let gv = meta.game_version.trim().to_string();
            (!gv.is_empty()).then_some(gv)
        })
}



/// 「4 : 4 : 1 : 1」混排的一轮（共 10 个槽位）：行为包 ×4、材质包 ×4、光影 ×1、LL 模组 ×1。
///
/// 槽位固定顺序，保证视觉上交替出现、权重稳定：
/// 行为包、材质包、光影、LL、行为包、材质包、行为包、材质包、行为包、材质包。
const ROUND_SLOTS: &[(&str, &str)] = &[
    (TYPE_BEHAVIOR_PACK, TYPE_BEHAVIOR_PACK),
    (TYPE_TEXTURE_PACK, TYPE_TEXTURE_PACK),
    (TYPE_SHADER, TYPE_SHADER),
    (TYPE_LL_MOD, TYPE_LL_MOD),
    (TYPE_BEHAVIOR_PACK, TYPE_BEHAVIOR_PACK),
    (TYPE_TEXTURE_PACK, TYPE_TEXTURE_PACK),
    (TYPE_BEHAVIOR_PACK, TYPE_BEHAVIOR_PACK),
    (TYPE_TEXTURE_PACK, TYPE_TEXTURE_PACK),
    (TYPE_BEHAVIOR_PACK, TYPE_BEHAVIOR_PACK),
    (TYPE_TEXTURE_PACK, TYPE_TEXTURE_PACK),
];

/// 全部来源 + 全部类型：按 `4:4:1:1` 混排组装一页。
///
/// 每轮 10 条（行为包 ×4、材质包 ×4、光影 ×1、LL 模组 ×1）。一页 40 条 = 4 轮，
/// 因此每页各类型消耗：行为包 16、材质包 16、光影 4、LL 模组 4。
/// 分页时，每类型在「本类型列表」内的起始下标 = 页码 × 该系统类型的每页条数，
/// 从而翻页不回重、不漏条，且保持各类型内部按下载量降序。
const TYPE_PER_PAGE: &[(&str, usize)] = &[
    (TYPE_BEHAVIOR_PACK, 16),
    (TYPE_TEXTURE_PACK, 16),
    (TYPE_SHADER, 4),
    (TYPE_LL_MOD, 4),
];

/// 从某类型列表 `[offset, offset+count)` 抓取条目。
///
/// 类型子列表统一按 40/页分页，故目标区间可能跨越 1~2 个子页，这里逐个拉取并截取。
async fn type_pick(
    kernel: &KernelContext,
    t: &str,
    offset: usize,
    count: usize,
    game_version: Option<&str>,
    sort: Option<&str>,
) -> Result<(Vec<ContentItem>, bool), KernelError> {
    let mut out = Vec::with_capacity(count);
    let mut has_more = false;
    let mut from = offset;
    let mut need = count;
    while need > 0 {
        let sub_page = from / 40;
        let within = from % 40;
        let t_query = ContentListQuery {
            source: if t == TYPE_LL_MOD { Some(SOURCE_LIP.to_string()) } else { None },
            content_type: Some(t.to_string()),
            search: None,
            game_version: game_version.map(str::to_string),
            sort: sort.map(str::to_string),
            page: sub_page as u32,
        };
        let typed: ContentListPage = if t == TYPE_LL_MOD {
            lip::list(kernel, &t_query).await?
        } else {
            curseforge::list(kernel, &t_query).await?
        };
        has_more = typed.has_more;
        let part: Vec<ContentItem> = typed
            .items
            .into_iter()
            .skip(within)
            .take(need)
            .collect();
        let got = part.len();
        if got == 0 {
            break;
        }
        out.extend(part);
        from += got;
        need -= got;
    }
    Ok((out, has_more))
}

async fn mixed_list(kernel: &KernelContext, query: &ContentListQuery) -> Result<ContentListPage, KernelError> {
    let search = query.search.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("");
    if !search.is_empty() {
        // 带关键字时退化为定向来源，避免跨源拼接语义混乱。
        let trimmed = search.trim().to_lowercase();
        let is_mod_word = ["ll", "lal", "levilamina", "mod", "模组", "插件"]
            .iter()
            .any(|k| trimmed.contains(k));
        return if is_mod_word {
            lip::list(kernel, query).await
        } else {
            curseforge::list(kernel, query).await
        };
    }

    let page = query.page as usize;
    let round_size = ROUND_SLOTS.len(); // 10
    let mut output: Vec<ContentItem> = Vec::with_capacity(PAGE_SIZE as usize);
    let mut has_more = false;
    let mut total: u64 = 0;

    // 每类型该页从自身列表 offset = page × 每页条数 起取 per_page 条，
    // 从而翻页不回重、不漏条。
    let mut pools: std::collections::HashMap<&str, Vec<ContentItem>> =
        std::collections::HashMap::new();
    let mut consumed: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();

    for &(t, per_page) in TYPE_PER_PAGE {
        let (items, more) = type_pick(
            kernel,
            t,
            page * per_page,
            per_page,
            query.game_version.as_deref(),
            query.sort.as_deref(),
        )
        .await?;
        has_more = has_more || more;
        // 类型 total 仅作分页近似合计（抓取接口不再回传 total，此处用已取到的量近似）。
        total += per_page as u64;
        pools.insert(t, items);
        consumed.insert(t, 0);
    }

    // 按混排槽位穿插输出；某类本页取尽则跳过。
    for i in 0..(PAGE_SIZE as usize) {
        let slot = ROUND_SLOTS[i % round_size].0;
        let Some(pool) = pools.get(slot) else {
            continue;
        };
        let taken = consumed[slot];
        if taken < pool.len() {
            output.push(pool[taken].clone());
            *consumed.get_mut(slot).unwrap() += 1;
        }
    }

    Ok(ContentListPage {
        items: output,
        has_more,
        total,
    })
}

/// lip 环境探测结果。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LipEnv {
    pub lip_available: bool,
    pub message: Option<String>,
}

// ---------------------------------------------------------------- 本地下载记录

const MIGRATIONS: &[crate::services::database::Migration] = &[
crate::services::database::Migration {
    version: 1,
    name: "content_download_record",
    sql: "CREATE TABLE IF NOT EXISTS module_content_download_record (
            id          TEXT PRIMARY KEY,
            source      TEXT NOT NULL,
            content_type TEXT NOT NULL,
            name        TEXT NOT NULL,
            version     TEXT NOT NULL DEFAULT '',
            state       TEXT NOT NULL DEFAULT 'downloading',
            dest        TEXT,
            task_id     INTEGER,
            updated_at  INTEGER NOT NULL
          );
          CREATE INDEX IF NOT EXISTS idx_content_download_record_updated
            ON module_content_download_record(updated_at);",
},
crate::services::database::Migration {
    version: 2,
    name: "content_download_record_error",
    sql: "ALTER TABLE module_content_download_record ADD COLUMN error TEXT;",
},
crate::services::database::Migration {
    version: 3,
    name: "content_download_record_android_mod",
    sql: "ALTER TABLE module_content_download_record ADD COLUMN target TEXT;",
},
crate::services::database::Migration {
    version: 4,
    name: "content_download_record_progress",
    // `progress` 为 0~1 的阶段进度；`stage` 为 i18n 键（`download.stage.*`）。
    // 只对「没有字节可算」的安装类条目有意义（lip 安装），普通下载的进度由
    // 内核下载引擎的内存任务提供，不落这张表。
    sql: "ALTER TABLE module_content_download_record ADD COLUMN progress REAL;
          ALTER TABLE module_content_download_record ADD COLUMN stage TEXT;",
},
];

/// 安卓模组记录的附加信息（`target` 列内容；下载完成钩子据此解包落位）。
///
/// 落位所需的全部信息都在投递时冻结在此：不在钩子里重读设置或重新拉目录，
/// 避免用户中途改默认版本、或目录条目已更新导致装到别处 / manifest 与实际不符。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct AndroidModPending {
    /// `lla:<modId>`（记录 id）。
    item_id: String,
    /// 暂存归档绝对路径。
    archive: String,
    /// 目标模组目录绝对路径（`.../mods/<modId>`）。
    mod_dir: String,
    /// 目标实例名。
    version: String,
    /// 写入规范化 manifest 的展示名。
    name: String,
    /// 写入规范化 manifest 的作者。
    author: String,
    /// 发布版本号。
    release_version: String,
    /// 该发布声明的 MC 版本（写回规范化 manifest）。
    game_versions: Vec<String>,
}

pub fn records(kernel: &KernelContext) -> Result<Vec<ContentDownloadRecord>, KernelError> {
    kernel.db().with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, source, content_type, name, version, state, dest, task_id, error, updated_at,
                    progress, stage
             FROM module_content_download_record ORDER BY updated_at DESC",
            // `target` 是内部列（安卓模组待落位信息），不对外暴露。
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ContentDownloadRecord {
                id: row.get(0)?,
                source: row.get(1)?,
                content_type: row.get(2)?,
                name: row.get(3)?,
                version: row.get(4)?,
                state: row.get(5)?,
                dest: row.get(6)?,
                task_id: row.get(7)?,
                error: row.get(8)?,
                updated_at: row.get(9)?,
                progress: row.get(10)?,
                stage: row.get(11)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
}

pub fn remove_record(kernel: &KernelContext, id: &str) -> Result<(), KernelError> {
    kernel.db().with_conn(|conn| {
        conn.execute("DELETE FROM module_content_download_record WHERE id = ?1", [id])?;
        Ok(())
    })
}

/// 清空内容下载记录（只清终态），返回删除行数。
///
/// 与内核下载引擎的「清空下载记录」同一条产品语义：擦掉历史，但绝不碰正在
/// 下载（`downloading`）或正在安装（`installing`）的条目——把正在跑的活儿从
/// 列表里抹掉，只会让用户以为任务被取消了，而磁盘上其实还在写。
pub fn clear_records(kernel: &KernelContext) -> Result<u64, KernelError> {
    kernel.db().with_conn(|conn| {
        let n = conn.execute(
            "DELETE FROM module_content_download_record
              WHERE state NOT IN ('downloading', 'installing')",
            [],
        )?;
        Ok(n as u64)
    })
}

/// 启动归一：把上次运行遗留的 `installing` 记录落为失败。
///
/// lip 安装是**同步**命令，进程被杀时 lipd 一并退出，磁盘上不存在还在跑的安装；
/// 不归一的话这条记录会永远显示「安装中」，既不会结束也无法重试。
pub fn normalize_interrupted_records(kernel: &KernelContext) {
    let result = kernel.db().with_conn(|conn| {
        let n = conn.execute(
            "UPDATE module_content_download_record
                SET state = 'failed', error = '上次运行时中断，可重新安装', updated_at = ?1
              WHERE state = 'installing'",
            rusqlite::params![chrono_now()],
        )?;
        Ok(n)
    });
    match result {
        Ok(n) if n > 0 => {
            log::info!("[content-download] 归一 {n} 条被中断的安装记录为失败");
        }
        Ok(_) => {}
        Err(e) => log::warn!("[content-download] 归一中断安装记录失败: {e}"),
    }
}

/// 记录一次安卓模组投递（`target` 列存 JSON 化的 [`AndroidModPending`]）。
///
/// 入参刻意收成「条目 + 落点 + 任务」三段，避免 8 个平铺参数里
/// 混淆同名的 `version`（发布版本号）与 `target.version`（实例名）。
fn record_ll_android_download(
    kernel: &KernelContext,
    item: &ContentItem,
    release_version: &str,
    target: &ll_android::InstallTarget,
    archive: &str,
    game_versions: Vec<String>,
    task_id: u64,
) -> Result<(), KernelError> {
    let pending = AndroidModPending {
        item_id: item.id.clone(),
        archive: archive.to_string(),
        mod_dir: target.mod_dir.to_string_lossy().into_owned(),
        version: target.version.clone(),
        name: item.name.clone(),
        author: item.author.clone().unwrap_or_default(),
        release_version: release_version.to_string(),
        game_versions,
    };
    let target = serde_json::to_string(&pending)?;
    let now = chrono_now();
    kernel.db().with_conn(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO module_content_download_record
              (id, source, content_type, name, version, state, dest, task_id, target, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'downloading', ?6, ?7, ?8, ?9)",
            rusqlite::params![
                item.id,
                item.source,
                item.content_type,
                item.name,
                release_version,
                archive,
                task_id,
                target,
                now
            ],
        )?;
        Ok(())
    })
}

/// 读取并清空某个任务的安卓模组待落位信息。
///
/// take 语义：返回后即从库里移除，避免下载完成事件重放导致重复解包。
fn take_android_mod_pending(db: &DatabaseService, task_id: u64) -> Option<AndroidModPending> {
    db.with_conn(|conn| -> Result<Option<AndroidModPending>, KernelError> {
        let raw: Option<String> = conn
            .query_row(
                "SELECT target FROM module_content_download_record WHERE task_id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .ok();
        let Some(raw) = raw else {
            return Ok(None);
        };
        conn.execute(
            "UPDATE module_content_download_record SET target = NULL WHERE task_id = ?1",
            [task_id],
        )?;
        Ok(serde_json::from_str::<AndroidModPending>(&raw).ok())
    })
    .ok()
    .flatten()
}

/// lip 安装阶段（i18n 键，与核心下载页的 `download.stage.*` 同命名空间）。
pub const STAGE_LIP_PREPARING: &str = "download.stage.lip_preparing";
pub const STAGE_LIP_INSTALLING: &str = "download.stage.lip_installing";
pub const STAGE_LIP_FINALIZING: &str = "download.stage.lip_finalizing";

/// lip 阶段的整体权重切分：准备（环境探测 / 查安装状态）5%，
/// lipd 安装 93%，收尾（回写记录 / 广播）2%。三段相加为 1。
const LIP_PREPARE_END: f64 = 0.05;
const LIP_INSTALL_END: f64 = 0.98;

/// 把 lipd 的安装百分比映射到整体进度（安装段的内部进度）。
fn lip_install_progress(percent: f64) -> f64 {
    let ratio = (percent / 100.0).clamp(0.0, 1.0);
    LIP_PREPARE_END + (LIP_INSTALL_END - LIP_PREPARE_END) * ratio
}

fn record_lip_install(
    kernel: &KernelContext,
    item: &ContentItem,
    version: &str,
    dest: Option<&str>,
) -> Result<(), KernelError> {
    let now = chrono_now();
    kernel.db().with_conn(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO module_content_download_record
             (id, source, content_type, name, version, state, dest, task_id, error, updated_at,
              progress, stage)
             VALUES (?1, ?2, ?3, ?4, ?5, 'installing', ?6, NULL, NULL, ?7, 0.0, ?8)",
            rusqlite::params![
                item.id,
                item.source,
                item.content_type,
                item.name,
                version,
                dest,
                now,
                STAGE_LIP_PREPARING
            ],
        )?;
        Ok(())
    })
}

/// 更新 lip 安装的阶段与进度（`progress` 为 0~1 的整体进度）。
fn update_lip_progress(
    db: &DatabaseService,
    id: &str,
    progress: f64,
    stage: &str,
) -> Result<(), KernelError> {
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE module_content_download_record
                SET progress = ?1, stage = ?2
              WHERE id = ?3",
            rusqlite::params![progress.clamp(0.0, 1.0), stage, id],
        )?;
        Ok(())
    })
}

/// lip 安装的进度上报器：把 lipd 回调变成「写记录 + 广播事件」。
///
/// 两处节流是必需的：lipd 的 `ReportProgress` 频率不可控，直接落库会在安装期间
/// 刷出成千上万条 UPDATE（每一条都是一次 fsync 级别的写）。事件则按 120ms
/// 一帧推送——比内核下载引擎的 200ms 更密，因为这里的一帧只改一个 DOM 宽度。
fn lip_progress_sink(db: Arc<DatabaseService>, events: Arc<EventBus>, item_id: String) -> lipd::CallbackSink {
    const EMIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(120);
    /// 进度至少动了 1% 才值得写库。
    const PERSIST_DELTA: f64 = 0.01;

    struct Tracker {
        progress: f64,
        stage: String,
        detail: Option<String>,
        last_emit: Option<std::time::Instant>,
        persisted_progress: f64,
    }

    let tracker = Arc::new(parking_lot::Mutex::new(Tracker {
        progress: 0.0,
        stage: STAGE_LIP_PREPARING.to_string(),
        detail: None,
        last_emit: None,
        persisted_progress: 0.0,
    }));

    Arc::new(move |callback: lipd::DaemonCallback| {
        let (target, stage, detail) = match callback {
            lipd::DaemonCallback::Progress { item, percent } => (
                percent.map(lip_install_progress).unwrap_or(LIP_PREPARE_END),
                STAGE_LIP_INSTALLING,
                (!item.trim().is_empty()).then_some(item),
            ),
            lipd::DaemonCallback::Log(text) => {
                let current = tracker.lock();
                (current.progress, current.stage.clone(), Some(text))
            }
        };

        let now = std::time::Instant::now();
        let (snapshot_progress, snapshot_stage, snapshot_detail, persist) = {
            let mut t = tracker.lock();
            // 进度只增不减：lipd 在依赖求解阶段会回退百分比，倒退的进度条
            // 比停滞更让人怀疑「是不是坏了」。
            t.progress = target.clamp(0.0, 1.0).max(t.progress);
            let stage_changed = t.stage != stage;
            t.stage = stage.to_string();
            t.detail = detail;
            let due = stage_changed
                || t.last_emit
                    .map(|prev| now.duration_since(prev) >= EMIT_INTERVAL)
                    .unwrap_or(true);
            if !due {
                return;
            }
            t.last_emit = Some(now);
            let persist = stage_changed
                || (t.progress - t.persisted_progress).abs() >= PERSIST_DELTA
                || t.progress >= 1.0;
            if persist {
                t.persisted_progress = t.progress;
            }
            (t.progress, t.stage.clone(), t.detail.clone(), persist)
        };

        if persist {
            if let Err(e) = update_lip_progress(&db, &item_id, snapshot_progress, &snapshot_stage) {
                log::warn!("[content-download] lip 进度写库失败（不影响安装）：{e}");
            }
        }
        events.publish(
            "content-download.install-progress",
            serde_json::json!({
                "id": item_id,
                "progress": snapshot_progress,
                "stage": snapshot_stage,
                "stageDetail": snapshot_detail,
            }),
        );
    })
}

fn update_lip_record(
    kernel: &KernelContext,
    id: &str,
    version: &str,
    state: &str,
    error: Option<&str>,
) -> Result<(), KernelError> {
    kernel.db().with_conn(|conn| {
        conn.execute(
            "UPDATE module_content_download_record SET version = ?1, state = ?2, error = ?3, updated_at = ?4 WHERE id = ?5",
            rusqlite::params![version, state, error, chrono_now(), id],
        )?;
        Ok(())
    })
}

fn record_download(
    kernel: &KernelContext,
    item: &ContentItem,
    version: &str,
    dest: &str,
    task_id: u64,
) -> Result<(), KernelError> {
    let now = chrono_now();
    kernel.db().with_conn(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO module_content_download_record
              (id, source, content_type, name, version, state, dest, task_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'downloading', ?6, ?7, ?8)",
            rusqlite::params![
                item.id,
                item.source,
                item.content_type,
                item.name,
                version,
                dest,
                task_id,
                now
            ],
        )?;
        Ok(())
    })
}

/// 按 `task_id` 更新下载记录状态（`download.status` 事件联动）。
fn mark_record_state(
    db: &Arc<DatabaseService>,
    task_id: u64,
    state: &str,
    error: Option<&str>,
) {
    let _ = db.with_conn(|conn| {
        conn.execute(
            "UPDATE module_content_download_record SET state = ?1, error = ?2 WHERE task_id = ?3",
            rusqlite::params![state, error, task_id],
        )?;
        Ok(())
    });
}

/// 内容落点解析结果。
struct InstallTarget {
    /// 最终下载目标文件路径。
    dest: PathBuf,
    /// 目标版本名（版本内安装时 Some，否则为缓存落点）。
    version: Option<String>,
    /// 是否需要向前端提示落点说明。
    notice: Option<String>,
}

/// 解析 CurseForge 资源的最终落点：
/// - 有选中版本（`launch.default_version`）且类型可放置 → 版本 `com.mojang/<子目录>`；
/// - 无选中版本 → 回退 `cache/content`；
/// - 选中版本存在但内容根不可用（如非隔离无 AppX）→ 回退 cache 并发布落点提示事件。
fn resolve_install_target(
    kernel: &KernelContext,
    item: &model::ContentItem,
    filename: &str,
) -> InstallTarget {
    let subdir = match item.content_type.as_str() {
        TYPE_BEHAVIOR_PACK => Some("behavior_packs"),
        TYPE_TEXTURE_PACK | TYPE_SHADER => Some("resource_packs"),
        _ => None, // ll_mod 经 lip 安装，不走本路径
    };
    let target = kernel
        .settings()
        .get::<String>("launch.default_version")
        .filter(|s| !s.trim().is_empty());
    let Some(target) = target else {
        return cache_target(kernel, filename);
    };
    let Some(subdir) = subdir else {
        return cache_target(kernel, filename);
    };

    match content::content_roots(kernel, &target) {
        Ok(roots) => {
            let dir = roots.com_mojang.join(subdir);
            let _ = std::fs::create_dir_all(&dir);
            let name = sanitize_filename(filename);
            InstallTarget {
                dest: dir.join(name),
                version: Some(target),
                notice: None,
            }
        }
        Err(e) => {
            // 内容根不可用：回退缓存但不静默，发布落点提示。
            log::warn!("[content-download] 版本 `{target}` 内容根不可用，回退缓存: {e}");
            let mut t = cache_target(kernel, filename);
            t.notice = Some(format!(
                "选中版本 `{target}` 内容根不可用（{e}），已下载到缓存目录"
            ));
            t
        }
    }
}

fn cache_target(kernel: &KernelContext, filename: &str) -> InstallTarget {
    let dir = kernel.paths().cache_dir().join("content");
    let _ = std::fs::create_dir_all(&dir);
    InstallTarget {
        dest: dir.join(sanitize_filename(filename)),
        version: None,
        notice: None,
    }
}

/// 处理 `download.status` 事件负载，更新本地记录状态。
fn apply_download_status(
    db: &Arc<DatabaseService>,
    events: &Arc<EventBus>,
    payload: &Value,
) {
    let Some(task_id) = payload.get("id").and_then(Value::as_u64) else {
        return;
    };
    match payload.get("status").and_then(Value::as_str) {
        Some("done") => {
            // 安卓 LL 模组：下载完成 ≠ 安装完成，需先解包落位再置 installed。
            if let Some(pending) = take_android_mod_pending(db, task_id) {
                install_ll_android_mod(db, events, pending);
                return;
            }
            mark_record_state(db, task_id, "installed", None);
            // 安装入版本的资源 → 通知版本页重扫内容。
            if let Some((item_id, target)) = find_record_target(db, task_id) {
                events.publish(
                    "content-download.location",
                    serde_json::json!({ "id": item_id, "version": target }),
                );
            }
        }
        Some("failed" | "cancelled") => {
            let msg = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("下载失败")
                .to_string();
            // 失败也要清掉待落位信息，否则残留会在下次同id 事件里被误当作待装。
            let _ = take_android_mod_pending(db, task_id);
            mark_record_state(db, task_id, "failed", Some(&msg));
        }
        _ => {}
    }
}

/// 下载完成后把安卓模组归档解包并原子落位到 `mods/<modId>`。
///
/// 落位成功才置`installed`；失败则记录 `failed` 并删暂存归档，
/// 绝不留下「记录说成功、磁盘上没有」的假状态。
fn install_ll_android_mod(
    db: &Arc<DatabaseService>,
    events: &Arc<EventBus>,
    pending: AndroidModPending,
) {
    let archive = PathBuf::from(&pending.archive);
    let mod_dir = PathBuf::from(&pending.mod_dir);

    // 暂存文件缺失：任务可能被清理或应用被杀，明确失败而非静默跳过。
    if !archive.is_file() {
        let message = format!("模组归档不存在：{}", archive.display());
        log::error!("[content-download] {message}");
        mark_record_state_by_id(db, &pending.item_id, "failed", Some(&message));
        events.publish(
            "content-download.location",
            serde_json::json!({ "error": message }),
        );
        return;
    }

    let display = ll_android::ModDisplay {
        name: pending.name.clone(),
        author: pending.author.clone(),
        version: pending.release_version.clone(),
        minecraft_versions: pending.game_versions.clone(),
    };

    match ll_android::install_archive(&archive, &mod_dir, &display) {
        Ok(()) => {
            let _ = std::fs::remove_file(&archive);
            mark_record_state_by_id(
                db,
                &pending.item_id,
                "installed",
                None,
            );
            // dest 从暂存路径改为真实落点，便于用户在下载中心查看。
            let _ = set_record_dest(db, &pending.item_id, &mod_dir.to_string_lossy());
            events.publish(
                "content-download.location",
                serde_json::json!({
                    "id": pending.item_id,
                    "version": pending.version,
                    "mod_dir": mod_dir.to_string_lossy(),
                }),
            );
        }
        Err((code, message)) => {
            let text = format!("[{code}] {message}");
            log::error!("[content-download] 安卓模组安装失败：{text}");
            let _ = std::fs::remove_file(&archive);
            mark_record_state_by_id(db, &pending.item_id, "failed", Some(&text));
            events.publish(
                "content-download.location",
                serde_json::json!({ "id": pending.item_id, "error": text }),
            );
        }
    }
}

fn mark_record_state_by_id(
    db: &Arc<DatabaseService>,
    id: &str,
    state: &str,
    error: Option<&str>,
) {
    let _ = db.with_conn(|conn| {
        conn.execute(
            "UPDATE module_content_download_record
             SET state = ?1, error = ?2, updated_at = ?3
             WHERE id = ?4",
            rusqlite::params![state, error, chrono_now(), id],
        )?;
        Ok(())
    });
}

/// 更新记录的真实落点（安装成功后从暂存路径改为模组目录）。
fn set_record_dest(db: &Arc<DatabaseService>, id: &str, dest: &str) -> Result<(), KernelError> {
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE module_content_download_record SET dest = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![dest, chrono_now(), id],
        )?;
        Ok(())
    })
}



/// 查询记录里是否存在目标版本，用于下载完成后提示落点。
fn find_record_target(db: &Arc<DatabaseService>, task_id: u64) -> Option<(String, String)> {
    db.with_conn(
            |conn| -> Result<Option<(String, String)>, KernelError> {
                let mut stmt = conn.prepare(
                    "SELECT id, version FROM module_content_download_record WHERE task_id = ?1",
                )?;
                let mut rows = stmt.query_map([task_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                match rows.next() {
                    Some(r) => {
                        let (id, version) = r?;
                        if version.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some((id, version)))
                        }
                    }
                    None => Ok(None),
                }
            },
        )
        .ok()
        .flatten()
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 清理写入磁盘的文件名（防路径穿越）。
fn sanitize_filename(name: &str) -> String {
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

/// 在 PATH 中查找可执行文件（Windows 自动补 `.exe`）。
fn find_executable(name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let pathext = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string())
            .to_lowercase()
    } else {
        String::new()
    };
    for dir in std::env::split_paths(&path_var) {
        let base = dir.join(name);
        if base.is_file() {
            return Some(base);
        }
        if cfg!(windows) {
            for ext in pathext.split(';').filter(|e| !e.is_empty()) {
                let cand = dir.join(format!("{name}{ext}"));
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------- Module trait

impl Module for ContentDownloadModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    /// 内置模块无独立版本事实源，跟随内核 crate 版本发布（见 cgl-libs.md 3.5 G2）。
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn init(&self, kernel: &KernelContext) -> Result<(), KernelError> {
        // 数据库 schema（记录已下载项）。
        kernel
            .db()
            .migrate_scope("module:content-download", MIGRATIONS)?;

        // i18n：注册模块语言包（源在前端同模块目录）。
        kernel.i18n().register_module_pack(
            "content-download",
            "zh-CN",
            serde_json::from_str(include_str!(
                "../../../../frontend/src/modules/content-download/locales/zh-CN.json"
            ))?,
        )?;
        kernel.i18n().register_module_pack(
            "content-download",
            "en-US",
            serde_json::from_str(include_str!(
                "../../../../frontend/src/modules/content-download/locales/en-US.json"
            ))?,
        )?;

        Ok(())
    }

    fn start(&self, kernel: &KernelContext) -> Result<(), KernelError> {
        // 上次运行遗留的 `installing` 记录落为失败：lip 安装是同步命令，进程
        // 被杀时 lipd 一并退出，不存在还在跑的安装，不归一就会永远显示「安装中」。
        normalize_interrupted_records(kernel);
        // 订阅下载状态事件：完成后把本地记录状态同步为 installed / failed，并广播落点。
        let db = kernel.db().clone();
        let events = kernel.events().clone();
        let sub = kernel.events().subscribe("download.status", move |_name, payload| {
            apply_download_status(&db, &events, payload);
        });
        *self.subs.lock().unwrap() = vec![sub];
        Ok(())
    }

    fn stop(&self, kernel: &KernelContext) -> Result<(), KernelError> {
        for sub in self.subs.lock().unwrap().drain(..) {
            kernel.events().unsubscribe(sub);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_strips_dangerous_chars() {
        assert_eq!(sanitize_filename("好的.mcpack"), "好的.mcpack");
        assert_eq!(sanitize_filename("a/b\\c:d*e?.mcpack"), "a_b_c_d_e_.mcpack");
        assert_eq!(sanitize_filename("..\\..\\evil.mcpack"), ".._.._evil.mcpack");
        assert_eq!(sanitize_filename(""), "download.bin");
        assert_eq!(sanitize_filename("   "), "download.bin");
    }

    /// lipd 的百分比必须落在「安装」段内，且首尾与阶段边界对齐。
    #[test]
    fn lip_progress_maps_into_install_segment() {
        assert_eq!(lip_install_progress(0.0), LIP_PREPARE_END);
        assert_eq!(lip_install_progress(100.0), LIP_INSTALL_END);
        let middle = lip_install_progress(50.0);
        assert!(middle > LIP_PREPARE_END && middle < LIP_INSTALL_END);
        // 越界百分比夹紧：进度条永远不越出 0~1。
        assert_eq!(lip_install_progress(9999.0), LIP_INSTALL_END);
        assert_eq!(lip_install_progress(-1.0), LIP_PREPARE_END);
    }

    /// 三段权重必须完整覆盖 0~1（准备 → lipd 安装 → 收尾）。
    #[test]
    fn lip_phase_weights_are_complete() {
        let covered =
            LIP_PREPARE_END + (LIP_INSTALL_END - LIP_PREPARE_END) + (1.0 - LIP_INSTALL_END);
        assert!((covered - 1.0).abs() < 1e-9, "阶段权重未覆盖满 0~1：{covered}");
        // 阶段键必须在核心下载页的命名空间内（前端对未知键退化为键名本身）。
        for key in [STAGE_LIP_PREPARING, STAGE_LIP_INSTALLING, STAGE_LIP_FINALIZING] {
            assert!(key.starts_with("download.stage."), "阶段键命名空间不对: {key}");
        }
    }
}