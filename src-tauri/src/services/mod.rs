//! 内核能力服务：路径、数据库、设置、i18n、主题、下载队列、账户、更新、商店授权、提示。
//!
//! 服务之间允许有限依赖（如设置被主题 / i18n 依赖，提示依赖 i18n），
//! 但不允许反向依赖命令层。

pub mod account;
pub mod database;
pub mod download;
pub mod http_client;
pub mod i18n;
pub mod paths;
pub mod settings;
pub mod native_install;
pub mod store_entitlement;
pub mod store_wam;
pub mod store_device;
pub mod store_rst;
pub mod store_rst_transport;
pub mod theme;
pub mod tips;
pub mod updater;
pub mod xal;
