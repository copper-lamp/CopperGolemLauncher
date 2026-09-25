//! 内容下载模块命令：列表 / 详情 / readme / 下载投递 / lip 安装。
//!
//! 所有命令为异步，错误经 `into_command_error` 统一映射，前端据此反馈。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::modules::content_download::model::{ContentDetail, ContentListPage, ContentListQuery};
use crate::modules::content_download::{ContentDownloadModule, LipEnv, LipInstallOutcome};
use crate::state::KernelContext;

/// 列表：按来源 / 类型过滤 + 关键字搜索 + 分页。
#[tauri::command]
pub async fn content_download_list(
    kernel: State<'_, KernelContext>,
    query: Option<ContentListQuery>,
) -> CommandResult<ContentListPage> {
    let query = query.unwrap_or_default();
    ContentDownloadModule::list(kernel.inner(), &query)
        .await
        .map_err(into_command_error)
}

/// 详情：按跨源 id（`cf:` / `lip:`）。
#[tauri::command]
pub async fn content_download_detail(
    kernel: State<'_, KernelContext>,
    id: String,
) -> CommandResult<ContentDetail> {
    ContentDownloadModule::detail(kernel.inner(), &id)
        .await
        .map_err(into_command_error)
}

/// 拉取项目 readme 原文（CF 为 HTML 片段、lip 为 Markdown）。
///
/// `locale` 由前端传入当前界面语言，lip 据此优先匹配 `README.<locale>.md`。
#[tauri::command]
pub async fn content_download_readme(
    kernel: State<'_, KernelContext>,
    id: String,
    locale: Option<String>,
) -> CommandResult<Option<String>> {
    let locale = locale.unwrap_or_default();
    let locale = if locale.trim().is_empty() {
        "en-US"
    } else {
        locale.as_str()
    };
    ContentDownloadModule::readme(kernel.inner(), &id, locale)
        .await
        .map_err(into_command_error)
}

/// 可选游戏版本列表（供前端「版本过滤」下拉）。
#[tauri::command]
pub async fn content_download_game_versions(
    kernel: State<'_, KernelContext>,
) -> CommandResult<Vec<String>> {
    ContentDownloadModule::game_versions(kernel.inner())
        .await
        .map_err(into_command_error)
}

/// 下载投递：CurseForge 文件直链 → 内核下载队列，返回任务 id。
#[tauri::command]
pub async fn content_download_download(
    kernel: State<'_, KernelContext>,
    id: String,
    file_id: String,
) -> CommandResult<u64> {
    ContentDownloadModule::download(kernel.inner(), &id, &file_id)
        .await
        .map_err(into_command_error)
}

/// 探测 lip 环境（是否安装 lipd 可执行文件）。
#[tauri::command]
pub async fn content_download_lip_env() -> CommandResult<LipEnv> {
    Ok(ContentDownloadModule::lip_env())
}

/// 经 lipd 安装 / 更新 LL 模组到目标版本目录。
///
/// `variant` 缺省按 lip 约定回退 `client`；`dir` 缺省解析设置 `launch.default_version`。
/// 域内失败以 `success=false` + `errorCode` 返回，不走命令错误通道。
#[tauri::command]
pub async fn content_download_lip_install(
    kernel: State<'_, KernelContext>,
    id: String,
    version: String,
    variant: Option<String>,
    dir: Option<String>,
) -> CommandResult<LipInstallOutcome> {
    Ok(ContentDownloadModule::lip_install(
        kernel.inner(),
        &id,
        &version,
        variant.as_deref(),
        dir.as_deref(),
    )
    .await)
}