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
/// 触控层布局（屏幕控件）的事实源：`<版本目录>/controls.json`。
pub mod controls;
/// 注入编排：hook DLL 落位 + 导入表改写 + 原始文件备份与还原。
pub mod inject;
/// 实例隔离与版本硬规则的唯一事实源（数据目录布局、编辑器门槛）。
pub mod isolate;
pub mod launch;
pub mod meta;
pub mod mods;
/// 启动后监控：确认成功、区分失败原因、观察生命周期。
pub mod monitor;
/// 预加载清单生成（`copper-preload.json`）。
pub mod preload;
/// 窗口：游戏窗口呈现判定 + 启动器窗口行为。
pub mod window;

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
    /// 恒为 `true`：隔离强制开启，没有关闭入口。仍然下发是为了让前端
    /// 能明确告诉用户「本实例的数据是独立的」，而不是让用户猜。
    pub enable_isolation: bool,
    pub enable_editor_mode: bool,
    /// 以控制台子系统启动（改写游戏 exe 的 PE Subsystem，启动链路消费）。
    pub enable_console: bool,
    /// 该游戏版本是否支持编辑器模式。不支持时 `enable_editor_mode` 会被忽略，
    /// 前端据此禁用开关并说明原因，而不是给一个必然启动失败的 `-Editor true`。
    pub editor_supported: bool,
    pub registered: bool,
    /// 该实例已安装的加载器版本（`None` = 未装）。
    pub loader: Option<String>,
    pub logo_data_url: Option<String>,
    /// 版本目录绝对路径（前端打开文件夹用）。
    pub folder: String,
}

/// 版本设置部分更新（仅覆盖提供的字段）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VersionMetaUpdate {
    pub enable_editor_mode: Option<bool>,
    pub enable_console: Option<bool>,
    pub launch_args: Option<String>,
    pub env_vars: Option<String>,
}

/// 版本设置中「打开目录」快捷方式的目标目录种类。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenDirKind {
    /// 版本目录（`<versions>/<name>`）。
    Version,
    /// 模组目录（`<版本目录>/mods`）。
    Mods,
    /// 存档目录（玩家数据目录下的 `minecraftWorlds`）。
    Worlds,
}

/// 解析「打开目录」快捷方式的目标路径，`create` 为真时确保目录存在。
pub fn resolve_open_dir(
    kernel: &KernelContext,
    name: &str,
    kind: OpenDirKind,
    create: bool,
) -> Result<std::path::PathBuf, KernelError> {
    match kind {
        OpenDirKind::Version => {
            let dir = meta::resolve_version_dir(&kernel.versions_root(), name)?;
            if !dir.is_dir() {
                return Err(KernelError::InvalidArgument(format!("版本 `{name}` 不存在")));
            }
            Ok(dir)
        }
        OpenDirKind::Mods => mods::mods_dir(kernel, name, create),
        OpenDirKind::Worlds => content::worlds_dir(kernel, name, create),
    }
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
        let root = kernel.versions_root();
        let mut metas = meta::scan_versions(&root);
        metas.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        metas.into_iter().map(|m| to_view(&root, m)).collect()
    }

    /// 单个版本视图。
    pub fn get_version(kernel: &KernelContext, name: &str) -> Result<VersionView, KernelError> {
        let root = kernel.versions_root();
        let dir = meta::resolve_version_dir(&root, name)?;
        let meta = VersionMeta::read(&dir)
            .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 不存在")))?;
        Ok(to_view(&root, meta))
    }

    /// 部分更新版本设置。
    pub fn save_meta(
        kernel: &KernelContext,
        name: &str,
        update: &VersionMetaUpdate,
    ) -> Result<VersionView, KernelError> {
        let root = kernel.versions_root();
        let dir = meta::resolve_version_dir(&root, name)?;
        let mut meta = VersionMeta::read(&dir)
            .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 不存在")))?;
        if let Some(v) = update.enable_editor_mode {
            meta.enable_editor_mode = v;
        }
        if let Some(v) = update.enable_console {
            meta.enable_console = v;
        }
        if let Some(v) = &update.launch_args {
            meta.launch_args = v.clone();
        }
        if let Some(v) = &update.env_vars {
            meta.env_vars = v.clone();
        }
        // 隔离强制开启：无论元数据里写的是什么，落盘的都是 true。
        isolate::enforce_isolation(&mut meta);
        VersionMeta::write(&dir, &meta)?;
        Ok(to_view(&root, meta))
    }

    /// 重命名版本（目录 + 元数据）。
    pub fn rename_version(
        kernel: &KernelContext,
        old_name: &str,
        new_name: &str,
    ) -> Result<VersionView, KernelError> {
        let root = kernel.versions_root();
        let old_dir = meta::resolve_version_dir(&root, old_name)?;
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err(KernelError::InvalidArgument("版本名不能为空".into()));
        }
        if !new_name.eq_ignore_ascii_case(old_name) {
            meta::validate_version_name(&root, new_name)?;
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
        Ok(to_view(&root, meta))
    }

    /// 删除版本（游戏运行中拒绝；成功后广播 `version.removed`）。
    pub fn delete_version(kernel: &KernelContext, name: &str) -> Result<(), KernelError> {
        let root = kernel.versions_root();
        let dir = meta::resolve_version_dir(&root, name)?;
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

    /// 该版本的游戏进程是否在运行（开始页按钮形态的依据）。
    pub fn game_running(kernel: &KernelContext, name: &str) -> Result<bool, KernelError> {
        let root = kernel.versions_root();
        let dir = meta::resolve_version_dir(&root, name)?;
        Ok(launch::is_process_running_at_path(&dir.join(launch::GAME_EXE)))
    }

    /// 结束该版本运行中的游戏进程，返回结束的进程数。
    pub fn kill_game(kernel: &KernelContext, name: &str) -> Result<usize, KernelError> {
        let root = kernel.versions_root();
        let dir = meta::resolve_version_dir(&root, name)?;
        launch::terminate_process_at_path(&dir.join(launch::GAME_EXE))
    }

    /// 启动文件状态（hook 是否落位、导入是否生效、原始备份是否存在）。
    pub fn launch_file_state(kernel: &KernelContext, name: &str) -> Result<inject::LaunchFileState, KernelError> {
        let root = kernel.versions_root();
        let dir = meta::resolve_version_dir(&root, name)?;
        let meta = VersionMeta::read(&dir)
            .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 不存在")))?;
        Ok(inject::launch_file_state(
            &dir,
            &launch::game_exe_path(&dir),
            &meta,
        ))
    }

    /// 还原未注入的原始启动文件。
    pub fn restore_launch_file(kernel: &KernelContext, name: &str) -> Result<(), KernelError> {
        let root = kernel.versions_root();
        let dir = meta::resolve_version_dir(&root, name)?;
        let exe = launch::game_exe_path(&dir);
        if launch::is_process_running_at_path(&exe) {
            return Err(KernelError::InvalidArgument("游戏运行中，无法还原启动文件".into()));
        }
        inject::restore_original(&exe)
    }

    /// 删除原始启动文件备份（释放磁盘）。
    pub fn delete_launch_backup(kernel: &KernelContext, name: &str) -> Result<(), KernelError> {
        let root = kernel.versions_root();
        let dir = meta::resolve_version_dir(&root, name)?;
        inject::delete_backup(&launch::game_exe_path(&dir))
    }

    /// 预加载清单探测摘要（版本设置页展示「将要加载什么」）。
    pub fn preload_summary(kernel: &KernelContext, name: &str) -> Result<preload::PreloadSummary, KernelError> {
        preload::detect(kernel, name)
    }
}

/// 构建前端视图（附带图标 data URL、加载器与目录路径）。
fn to_view(root: &std::path::Path, meta: VersionMeta) -> VersionView {
    let dir = root.join(&meta.name);
    VersionView {
        editor_supported: isolate::supports_editor_mode(&meta.game_version, &meta.version_type),
        name: meta.name.clone(),
        game_version: meta.game_version.clone(),
        version_type: meta.version_type.clone(),
        // 隔离强制开启：元数据里的 false 一律按 true 对外。
        enable_isolation: true,
        enable_editor_mode: meta.enable_editor_mode,
        enable_console: meta.enable_console,
        registered: meta.registered,
        loader: meta.loader.clone(),
        logo_data_url: meta::logo_data_url(&dir),
        folder: dir.to_string_lossy().into_owned(),
    }
}

impl Module for HomeModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    /// 内置模块无独立版本事实源，跟随内核 crate 版本发布（见 cgl-libs.md 3.5 G2）。
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
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
        let settings = kernel.settings().clone();
        let expose_handler: IntentHandler = Arc::new(move |_payload| {
            let metas = meta::scan_versions(&paths.versions_root(&settings));
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
        // 游戏目录变更 → 广播版本清单变更，前端各版本页据此重扫。
        let events = kernel.events().clone();
        let sub_c = kernel.events().subscribe("settings.changed", move |_name, payload| {
            if let Some(v) = payload.get("game.directory") {
                events.publish(
                    "versions.changed",
                    serde_json::json!({ "gameDirectory": v }),
                );
            }
        });
        *self.subs.lock().unwrap() = vec![sub_a, sub_b, sub_c];
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
        let update = VersionMetaUpdate {
            launch_args: Some("-x".into()),
            ..Default::default()
        };
        let mut meta = VersionMeta {
            name: "t".into(),
            enable_editor_mode: true,
            ..Default::default()
        };
        if let Some(v) = &update.launch_args {
            meta.launch_args = v.clone();
        }
        assert_eq!(meta.launch_args, "-x");
        // 未提供的字段保持不变
        assert!(meta.enable_editor_mode);
        assert!(!meta.enable_console);
    }

    /// 隔离强制开启：即便元数据里是 false，落盘前也必须被归一为 true。
    #[test]
    fn saving_meta_forces_isolation_on() {
        let mut meta = VersionMeta {
            name: "t".into(),
            enable_isolation: false,
            ..Default::default()
        };
        assert!(isolate::enforce_isolation(&mut meta));
        assert!(meta.enable_isolation);
    }

    /// 视图恒报隔离开启，且带上加载器与编辑器门槛。
    #[test]
    fn view_reports_isolation_loader_and_editor_support() {
        let view = to_view(
            std::path::Path::new("D:/v"),
            VersionMeta {
                name: "t".into(),
                game_version: "1.21.40.10".into(),
                version_type: "release".into(),
                enable_isolation: false,
                loader: Some("0.16.2".into()),
                ..Default::default()
            },
        );
        // 元数据写的是 false，对外仍然是 true
        assert!(view.enable_isolation);
        assert_eq!(view.loader.as_deref(), Some("0.16.2"));
        // 1.21.40 低于正式版编辑器门槛 1.21.50
        assert!(!view.editor_supported);

        let preview = to_view(
            std::path::Path::new("D:/v"),
            VersionMeta {
                name: "t".into(),
                game_version: "1.19.80.20".into(),
                version_type: "preview".into(),
                ..Default::default()
            },
        );
        assert!(preview.editor_supported);
    }
}
