//! 内容下载模块（`content-download`）：拉取网络内容、浏览并下载。
//!
//! 两类数据源：
//! - **CurseForge**（行为包 / 材质包 / 光影包）：列表 / 详情 / 文件直链齐备，
//!   `download` 把 `downloadUrl` 投递到内核全局下载队列（悬浮窗统一展示进度）。
//! - **LIP**（LL 模组）：lipr 索引导仅含元数据，真实安装依赖用户预装的 lip + BDS，
//!   经 `lip_install` 调用 lip 子进程完成；不伪造直链。
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
    TYPE_BEHAVIOR_PACK, TYPE_LL_MOD, TYPE_SHADER, TYPE_TEXTURE_PACK,
};

pub mod curseforge;
pub mod http;
pub mod lip;
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
        // 全部来源 + 全部类型 → 四类混排。
        let result = if source.is_none() || source == Some("") {
            if ctype.is_none() || ctype == Some("") {
                mixed_list(kernel, query).await
            } else if ctype == Some(TYPE_LL_MOD) {
                lip::list(kernel, query).await
            } else {
                curseforge::list(kernel, query).await
            }
        } else if source == Some(SOURCE_LIP) || ctype == Some(TYPE_LL_MOD) {
            lip::list(kernel, query).await
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

    /// 详情：按跨源 id（`cf:` / `lip:` 前缀）路由。
    pub async fn detail(kernel: &KernelContext, id: &str) -> Result<ContentDetail, KernelError> {
        if let Some(cf_id) = id.strip_prefix("cf:") {
            let mod_id: i64 = cf_id
                .parse()
                .map_err(|_| KernelError::InvalidArgument(format!("无效内容 id `{id}`")))?;
            curseforge::detail(kernel, mod_id).await
        } else if let Some(ident) = id.strip_prefix("lip:") {
            lip::detail(kernel, ident).await
        } else {
            Err(KernelError::InvalidArgument(format!("无法识别的内容 id `{id}`")))
        }
    }

    /// 拉取 CurseForge 项目 readme（HTML）。仅 CF 支持。
    pub async fn readme(kernel: &KernelContext, id: &str) -> Result<Option<String>, KernelError> {
        let cf_id = id
            .strip_prefix("cf:")
            .ok_or_else(|| KernelError::InvalidArgument("readme 仅支持 CurseForge 内容".into()))?;
        let mod_id: i64 = cf_id
            .parse()
            .map_err(|_| KernelError::InvalidArgument(format!("无效内容 id `{id}`")))?;
        curseforge::description(kernel, mod_id).await
    }

    /// 下载投递：CurseForge 文件直链 → 内核下载队列，并落库一条下载记录。
    ///
    /// LIP 无直链，返回明确错误提示需经 lip 安装。
    pub async fn download(
        kernel: &KernelContext,
        id: &str,
        file_id: &str,
    ) -> Result<u64, KernelError> {
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

    /// 探测 lip 环境：lip 可执行是否可用、应用版本目录。
    pub async fn lip_env() -> LipEnv {
        LipEnv {
            lip_available: find_executable("lip").is_some(),
            message: if find_executable("lip").is_some() {
                None
            } else {
                Some("未检测到 lip，请先安装 lip（并准备 BDS 环境）".to_string())
            },
        }
    }

    /// 经 lip 安装 LL 模组：在目标版本目录执行 `lip install <ident>@<version> -y`。
    pub async fn lip_install(
        kernel: &KernelContext,
        id: &str,
        version: &str,
        dir: Option<String>,
    ) -> Result<LipInstallOutcome, KernelError> {
        let ident = id
            .strip_prefix("lip:")
            .ok_or_else(|| KernelError::InvalidArgument("仅支持 lip 来源的模组安装".into()))?;
        let lip = find_executable("lip")
            .ok_or_else(|| KernelError::InvalidArgument("未检测到 lip，请先安装 lip（并准备 BDS 环境）".into()))?;

        let cwd = match dir {
            Some(d) if !d.trim().is_empty() => d,
            _ => kernel.paths().versions_dir().to_string_lossy().into_owned(),
        };

        let pkg = format!("{ident}@{version}");
        let output = tokio::process::Command::new(&lip)
            .arg("install")
            .arg(&pkg)
            .arg("-y")
            .current_dir(&cwd)
            .output()
            .await?;

        Ok(LipInstallOutcome {
            success: output.status.success(),
            package: pkg,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
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
        let (items, more) = type_pick(kernel, t, page * per_page, per_page).await?;
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

/// lip 安装结果。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LipInstallOutcome {
    pub success: bool,
    pub package: String,
    pub stdout: String,
    pub stderr: String,
}

// ---------------------------------------------------------------- 本地下载记录

const MIGRATION: crate::services::database::Migration = crate::services::database::Migration {
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
};

fn record_download(
    kernel: &KernelContext,
    item: &model::ContentItem,
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
            mark_record_state(db, task_id, "failed", Some(&msg));
        }
        _ => {}
    }
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

    fn init(&self, kernel: &KernelContext) -> Result<(), KernelError> {
        // 数据库 schema（记录已下载项）。
        kernel
            .db()
            .migrate_scope("module:content-download", &[MIGRATION])?;

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
}