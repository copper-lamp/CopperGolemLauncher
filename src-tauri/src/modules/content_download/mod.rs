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

use copper_downloader::DownloadOptions;

use crate::error::KernelError;
use crate::registry::modules::Module;
use crate::state::KernelContext;

use self::model::{ContentDetail, ContentListPage, ContentListQuery, SOURCE_LIP, TYPE_LL_MOD};

pub mod curseforge;
pub mod http;
pub mod lip;
pub mod model;

/// 模块唯一标识。
pub const MODULE_ID: &str = "content-download";

/// 内容下载模块实例。
pub struct ContentDownloadModule;

impl Default for ContentDownloadModule {
    fn default() -> Self {
        Self
    }
}

// ---------------------------------------------------------------- 对外业务

impl ContentDownloadModule {
    /// 列表：按来源 / 类型过滤 + 关键字搜索 + 分页。
    pub async fn list(
        kernel: &KernelContext,
        query: &ContentListQuery,
    ) -> Result<ContentListPage, KernelError> {
        let source = query.source.as_deref();
        let ctype = query.content_type.as_deref();
        // 明确指定 LIP，或未指定来源但类型为 ll_mod → 走 LIP；否则走 CurseForge。
        if source == Some(SOURCE_LIP) || (source.is_none() && ctype == Some(TYPE_LL_MOD)) {
            lip::list(kernel, query).await
        } else {
            curseforge::list(kernel, query).await
        }
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

        // 保存到模块下载目录（cache/content，按文件名）。
        let dir = kernel.paths().cache_dir().join("content");
        std::fs::create_dir_all(&dir)?;
        let dest = dir.join(sanitize_filename(&file.filename));

        let mut opts = DownloadOptions::default();
        opts.filename = Some(file.filename.clone());
        opts.resume = true;
        opts.expected_sha256 = file.sha256.clone();

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
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
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

    fn start(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
        Ok(())
    }

    fn stop(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
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