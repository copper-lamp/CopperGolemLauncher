//! 游戏下载模块（`game-download`）：拉取 MCBE 版本清单，下载版本包并安装进开始页。
//!
//! 数据源：社区维护 GDK 清单（三镜像 CDN），`.msixvc` 用内嵌闭源原生 DLL 解包
//! （需用户 Store 授权），历史 `.appx` 走 zip 回退。安装完成写 `version.json`，
//! 使版本进入「开始页」可启动项，并经事件总线广播 `version.installed`。
//!
//! 联动（经内核中介）：
//! - 订阅 `download.status`（内核全局下载队列）驱动 下载完成→安装；
//! - 持久化 `module:game-download`（下载任务→版本，支持重启续装）；
//! - 复用 `home::meta` 写版本元数据；发布 `version.installed` / `version.download_failed`。

use std::sync::{Arc, Mutex};

use crate::error::KernelError;
use crate::registry::events::Subscription;
use crate::registry::modules::Module;
use crate::state::KernelContext;

pub mod extractor;
pub mod installer;
pub mod manifest;
pub mod meta_bridge;

use installer::Ctx;

/// 模块唯一标识。
pub const MODULE_ID: &str = "game-download";

/// 游戏下载模块实例。
pub struct GameDownloadModule {
    /// 事件订阅句柄（stop 时退订）。
    subs: Mutex<Vec<Subscription>>,
    /// 安装单飞锁（一次只解一个包，避免并发 GB 级解压）。
    install_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Default for GameDownloadModule {
    fn default() -> Self {
        Self::new()
    }
}

impl GameDownloadModule {
    pub fn new() -> Self {
        Self {
            subs: Mutex::new(Vec::new()),
            install_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

impl Module for GameDownloadModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    fn init(&self, kernel: &KernelContext) -> Result<(), KernelError> {
        // 数据库 schema（下载任务→版本）。
        kernel
            .db()
            .migrate_scope("module:game-download", &[installer::MIGRATION])?;

        // i18n：注册模块语言包（源在前端同模块目录）。
        kernel.i18n().register_module_pack(
            "game-download",
            "zh-CN",
            serde_json::from_str(include_str!(
                "../../../../frontend/src/modules/game-download/locales/zh-CN.json"
            ))?,
        )?;
        kernel.i18n().register_module_pack(
            "game-download",
            "en-US",
            serde_json::from_str(include_str!(
                "../../../../frontend/src/modules/game-download/locales/en-US.json"
            ))?,
        )?;

        // 订阅：全局下载队列状态 → 驱动 下载完成→安装 / 失败落库。
        let ctx = Ctx::from_kernel(kernel);
        let lock = self.install_lock.clone();
        let sub = kernel.events().subscribe("download.status", move |_name, payload| {
            installer::handle_status(&ctx, &lock, payload);
        });
        *self.subs.lock().unwrap() = vec![sub];

        Ok(())
    }

    fn start(&self, kernel: &KernelContext) -> Result<(), KernelError> {
        // 确保缓存目录存在（下载 / DLL 落盘）。
        let ctx = Ctx::from_kernel(kernel);
        std::fs::create_dir_all(ctx.cache_home())?;
        std::fs::create_dir_all(ctx.gdk_dir())?;
        // 续装 / 续传未完成任务。
        installer::resume_pending(&ctx, &self.install_lock);
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
    fn module_id_stable() {
        assert_eq!(GameDownloadModule::new().id(), "game-download");
    }
}