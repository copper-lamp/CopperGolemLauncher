//! 内容下载模块命令：列表 / 详情 / readme / 下载投递 / lip 安装。
//!
//! 所有命令为异步，错误经 `into_command_error` 统一映射，前端据此反馈。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::modules::content_download::model::{ContentDetail, ContentListPage, ContentListQuery};
use crate::modules::content_download::{
    ContentDownloadModule, ContentDownloadRecord, ContentPlacement, LipEnv, LipInstallOutcome,
};
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

/// 落点预演：不投递，只解析这个文件下载后会落到哪。
///
/// 前端据此决定是否弹「当前无 MC 实例，内容将下载到 X」的确认框。
#[tauri::command]
pub async fn content_download_plan(
    kernel: State<'_, KernelContext>,
    id: String,
    file_id: String,
    version: Option<String>,
) -> CommandResult<ContentPlacement> {
    ContentDownloadModule::plan(kernel.inner(), &id, &file_id, version.as_deref())
        .await
        .map_err(into_command_error)
}

/// 下载投递：CurseForge 文件直链 → 内核下载队列，返回任务 id。
///
/// `version` 为 [`content_download_plan`] 解析出的目标版本，钉住下发以免
/// 两次调用之间用户改了开始页选择、导致前后端算到不同落点。
#[tauri::command]
pub async fn content_download_download(
    kernel: State<'_, KernelContext>,
    id: String,
    file_id: String,
    version: Option<String>,
) -> CommandResult<u64> {
    ContentDownloadModule::download(kernel.inner(), &id, &file_id, version.as_deref())
        .await
        .map_err(into_command_error)
}

#[tauri::command]
pub fn content_download_records(
    kernel: State<'_, KernelContext>,
) -> CommandResult<Vec<ContentDownloadRecord>> {
    crate::modules::content_download::records(kernel.inner()).map_err(into_command_error)
}

#[tauri::command]
pub fn content_download_record_remove(
    kernel: State<'_, KernelContext>,
    id: String,
) -> CommandResult<()> {
    crate::modules::content_download::remove_record(kernel.inner(), &id)
        .map_err(into_command_error)
}

/// 清空内容下载记录（只清终态），返回删除行数。
#[tauri::command]
pub fn content_download_records_clear(kernel: State<'_, KernelContext>) -> CommandResult<u64> {
    crate::modules::content_download::clear_records(kernel.inner()).map_err(into_command_error)
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