//! 内置模块后端挂载点：静态编译进内核的模块在此注册。
//!
//! 当前内置模块：`home`（开始页）、`content-download`（内容下载）。
//! 游戏下载模块后续在此挂载。模块契约见 [`crate::registry::modules::Module`]。

pub mod content_download;
pub mod home;
