//! 开始页模块（`home`）：启动游戏、版本清单管理、版本设置、内容管理。
//!
//! 联动（经内核中介）：
//! - 声明意图 `expose.version`（向其他模块提供版本列表）、`launch.game`（触发启动游戏）；
//! - 订阅事件 `version.installed` / `version.removed`（安装 / 删除后前端据此刷新清单）；
//! - 发布事件 `version.removed`（删除版本时广播）、`game.launched`（启动成功后广播）。

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::KernelError;
use crate::registry::events::Subscription;
use crate::registry::intents::IntentHandler;
use crate::registry::modules::Module;
use crate::state::KernelContext;

pub mod content;
pub mod launch;
pub mod meta;

use meta::VersionMeta;

/// 模块唯一标识。
pub const MODULE_ID: &str = "home";

/// 版本清单视图（供前端展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct VersionView {
    pub name: String,
    pub game_version: String,
    pub version_type: String,
    pub enable_isolation: bool,
    pub enable_editor_mode: bool,
    pub enable_render_dragon: bool,
    pub registered: bool,
    pub logo_data_url: Option<String>,
    /// 版本目录绝对路径（前端打开文件夹用）。
    pub folder: String,
}

/// 版本设置部分更新（仅覆盖提供的字段）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VersionMetaUpdate {
    pub enable_editor_mode: Option<bool>,
    pub enable_render_dragon: Option<bool>,
    pub enable_console: Option<bool>,
    pub enable_ctrl_r_reload_resources: Option<bool>,
    pub launch_args: Option<String>,
    pub env_vars: Option<String>,
}

/// 开始页模块实例。
pub struct HomeModule {
    /// 事件订阅句柄（stop 时退订）。
    subs: Mutex<Vec<Subscription>>,
}

impl Default for HomeModule {
    fn default() -> Self {
        Self::new()
    }
}

impl HomeModule {
    pub fn new() -> Self {
        Self {
            subs: Mutex::new(Vec::new()),
        }
    }

    /// 版本清单（实时扫描；`version.installed` / `version.removed` 事件直达前端触发刷新）。
    pub fn list_versions(kernel: &KernelContext) -> Vec<VersionView> {
        let root = kernel.paths().versions_dir();
        let mut metas = meta::scan_versions(root);
        metas.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        metas.into_iter().map(|m| to_view(root, m)).collect()
    }

    /// 单个版本视图。
    pub fn get_version(kernel: &KernelContext, name: &str) -> Result<VersionView, KernelError> {
        let root = kernel.paths().versions_dir();
        let dir = meta::resolve_version_dir(root, name)?;
        let meta = VersionMeta::read(&dir)
            .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 不存在")))?;
        Ok(to_view(root, meta))
    }

    /// 部分更新版本设置。
    pub fn save_meta(
        kernel: &KernelContext,
        name: &str,
        update: &VersionMetaUpdate,
    ) -> Result<VersionView, KernelError> {
        let root = kernel.paths().versions_dir();
        let dir = meta::resolve_version_dir(root, name)?;
        let mut meta = VersionMeta::read(&dir)
            .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 不存在")))?;
        if let Some(v) = update.enable_editor_mode {
            meta.enable_editor_mode = v;
        }
        if let Some(v) = update.enable_render_dragon {
            meta.enable_render_dragon = v;
        }
        if let Some(v) = update.enable_console {
            meta.enable_console = v;
        }
        if let Some(v) = update.enable_ctrl_r_reload_resources {
            meta.enable_ctrl_r_reload_resources = v;
        }
        if let Some(v) = &update.launch_args {
            meta.launch_args = v.clone();
        }
        if let Some(v) = &update.env_vars {
            meta.env_vars = v.clone();
        }
        VersionMeta::write(&dir, &meta)?;
        Ok(to_view(root, meta))
    }

    /// 重命名版本（目录 + 元数据）。
    pub fn rename_version(
        kernel: &KernelContext,
        old_name: &str,
        new_name: &str,
    ) -> Result<VersionView, KernelError> {
        let root = kernel.paths().versions_dir();
        let old_dir = meta::resolve_version_dir(root, old_name)?;
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err(KernelError::InvalidArgument("版本名不能为空".into()));
        }
        if !new_name.eq_ignore_ascii_case(old_name) {
            meta::validate_version_name(root, new_name)?;
        }
        let new_dir = root.join(new_name);
        if new_dir.exists() && new_dir != old_dir {
            return Err(KernelError::InvalidArgument("同名版本已存在".into()));
        }
        let mut meta = VersionMeta::read(&old_dir)
            .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{old_name}` 不存在")))?;
        if old_dir != new_dir {
            std::fs::rename(&old_dir, &new_dir)?;
        }
        meta.name = new_name.to_string();
        VersionMeta::write(&new_dir, &meta)?;
        Ok(to_view(root, meta))
    }

    /// 删除版本（游戏运行中拒绝；成功后广播 `version.removed`）。
    pub fn delete_version(kernel: &KernelContext, name: &str) -> Result<(), KernelError> {
        let root = kernel.paths().versions_dir();
        let dir = meta::resolve_version_dir(root, name)?;
        if !dir.exists() {
            return Err(KernelError::InvalidArgument(format!("版本 `{name}` 不存在")));
        }
        let exe = dir.join(launch::GAME_EXE);
        if launch::is_process_running_at_path(&exe) {
            return Err(KernelError::InvalidArgument("游戏运行中，无法删除".into()));
        }
        std::fs::remove_dir_all(&dir)?;
        kernel
            .events()
            .publish("version.removed", serde_json::json!({ "name": name }));
        Ok(())
    }

    /// 启动游戏（前端入口；意图 `launch.game` 也走此逻辑）。
    pub fn launch(
        kernel: &KernelContext,
        name: &str,
    ) -> Result<launch::LaunchOutcome, KernelError> {
        launch::launch_game(&launch::LaunchCtx::from_kernel(kernel), name, true)
    }
}

/// 构建前端视图（附带图标 data URL 与目录路径）。
fn to_view(root: &std::path::Path, meta: VersionMeta) -> VersionView {
    let dir = root.join(&meta.name);
    VersionView {
        name: meta.name.clone(),
        game_version: meta.game_version.clone(),
        version_type: meta.version_type.clone(),
        enable_isolation: meta.enable_isolation,
        enable_editor_mode: meta.enable_editor_mode,
        enable_render_dragon: meta.enable_render_dragon,
        registered: meta.registered,
        logo_data_url: meta::logo_data_url(&dir),
        folder: dir.to_string_lossy().into_owned(),
    }
}

impl Module for HomeModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    fn init(&self, kernel: &KernelContext) -> Result<(), KernelError> {
        // i18n：注册模块语言包（数据源在前端目录，后端 include_str 同源）。
        kernel.i18n().register_module_pack(
            "home",
            "zh-CN",
            serde_json::from_str(include_str!(
                "../../../../frontend/src/modules/home/locales/zh-CN.json"
            ))?,
        )?;
        kernel.i18n().register_module_pack(
            "home",
            "en-US",
            serde_json::from_str(include_str!(
                "../../../../frontend/src/modules/home/locales/en-US.json"
            ))?,
        )?;

        // 意图：expose.version —— 向其他模块提供版本列表。
        let paths = kernel.paths().clone();
        let expose_handler: IntentHandler = Arc::new(move |_payload| {
            let metas = meta::scan_versions(paths.versions_dir());
            let list: Vec<Value> = metas
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "name": m.name,
                        "game_version": m.game_version,
                        "type": m.version_type,
                        "registered": m.registered,
                    })
                })
                .collect();
            Ok(Value::Array(list))
        });
        kernel.intents().declare("expose.version", MODULE_ID, expose_handler)?;

        // 意图：launch.game —— 供其他模块触发启动游戏。
        let ctx = launch::LaunchCtx::from_kernel(kernel);
        let launch_handler: IntentHandler = Arc::new(move |payload| {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| KernelError::InvalidArgument("缺少版本名".into()))?;
            launch::launch_game(&ctx, name, true)?;
            Ok(serde_json::json!({ "ok": true }))
        });
        kernel.intents().declare("launch.game", MODULE_ID, launch_handler)?;

        // 订阅：版本安装 / 删除事件（游戏下载模块发布）。清单实时扫描，
        // 前端监听同名事件即可自动刷新；此处订阅验证链路并留扩展点。
        let sub_a = kernel.events().subscribe("version.installed", |name, payload| {
            log::info!("[home] event {name}: {payload}");
        });
        let sub_b = kernel.events().subscribe("version.removed", |name, payload| {
            log::info!("[home] event {name}: {payload}");
        });
        *self.subs.lock().unwrap() = vec![sub_a, sub_b];
        Ok(())
    }

    fn start(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
        Ok(())
    }

    fn stop(&self, kernel: &KernelContext) -> Result<(), KernelError> {
        for sub in self.subs.lock().unwrap().drain(..) {
            kernel.events().unsubscribe(sub);
        }
        kernel.intents().withdraw(MODULE_ID);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_meta_mutates_only_given_fields() {
        let mut update = VersionMetaUpdate::default();
        update.enable_render_dragon = Some(true);
        update.launch_args = Some("-x".into());
        let mut meta = VersionMeta {
            name: "t".into(),
            enable_editor_mode: true,
            ..Default::default()
        };
        if let Some(v) = update.enable_render_dragon {
            meta.enable_render_dragon = v;
        }
        if let Some(v) = &update.launch_args {
            meta.launch_args = v.clone();
        }
        assert!(meta.enable_render_dragon);
        assert_eq!(meta.launch_args, "-x");
        // 未提供的字段保持不变
        assert!(meta.enable_editor_mode);
        assert!(!meta.enable_console);
    }
}
