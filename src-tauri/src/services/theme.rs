//! 主题服务：深浅色模式（dark / light / auto）与强调色。
//!
//! 令牌本身由前端消费 `var(--copper-*)` 渲染；本服务负责校验与持久化设置，
//! 前端通过 `settings.changed` 事件实时响应。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::KernelError;
use crate::services::settings::SettingsService;

/// 外观模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Dark,
    Light,
    Auto,
}

/// 主题服务。
pub struct ThemeService {
    settings: Arc<SettingsService>,
}

impl ThemeService {
    pub fn new(settings: Arc<SettingsService>) -> Self {
        Self { settings }
    }

    pub fn mode(&self) -> ThemeMode {
        self.settings
            .get::<ThemeMode>("theme.mode")
            .unwrap_or(ThemeMode::Auto)
    }

    pub fn set_mode(&self, mode: ThemeMode) -> Result<(), KernelError> {
        self.settings.set("theme.mode", &mode)
    }

    /// 当前强调色（#RRGGBB）。
    pub fn accent(&self) -> String {
        self.settings
            .get::<String>("theme.accent")
            .filter(|s| is_valid_hex_color(s))
            .unwrap_or_else(|| "#3b82f6".to_string())
    }

    pub fn set_accent(&self, hex: &str) -> Result<(), KernelError> {
        if !is_valid_hex_color(hex) {
            return Err(KernelError::InvalidArgument(format!(
                "invalid accent color: {hex}（需 #RRGGBB）"
            )));
        }
        self.settings.set("theme.accent", &hex.to_lowercase())
    }

    /// 主题快照（供前端启动时一次性应用）。
    pub fn snapshot(&self) -> Value {
        serde_json::json!({
            "mode": self.mode(),
            "accent": self.accent(),
        })
    }
}

/// 校验 `#RRGGBB` 颜色。
pub fn is_valid_hex_color(s: &str) -> bool {
    let s = s.trim();
    let hex = s.strip_prefix('#').unwrap_or(s);
    hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())
}
