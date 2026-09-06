//! i18n 服务：多语言文案、语言切换、模块语言包合并。
//!
//! - 基准语言包嵌入 `frontend/src/locales/*.json`（单一数据源，前后端共享）。
//! - 回退规则：目标语言缺失 → en-US → 返回键名。
//! - 模块语言包以 `module.<模块名>.*` 命名空间注册，查询时与基准包合并。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde_json::Value;

use crate::error::KernelError;
use crate::services::settings::SettingsService;

const FALLBACK_LOCALE: &str = "en-US";

/// 支持的基准语言。语言包文件位于 `frontend/src/locales/`。
const BASE_CATALOGS: &[(&str, &str)] = &[
    (
        "zh-CN",
        include_str!("../../../frontend/src/locales/zh-CN.json"),
    ),
    (
        "en-US",
        include_str!("../../../frontend/src/locales/en-US.json"),
    ),
];

/// i18n 服务。
pub struct I18nService {
    settings: Arc<SettingsService>,
    /// locale -> 基准包
    base: HashMap<String, Value>,
    /// (模块名, locale) -> 模块语言包
    module_packs: RwLock<HashMap<(String, String), Value>>,
}

impl I18nService {
    pub fn new(settings: Arc<SettingsService>) -> Result<Self, KernelError> {
        let mut base = HashMap::new();
        for (locale, raw) in BASE_CATALOGS {
            base.insert(
                (*locale).to_string(),
                serde_json::from_str(raw)
                    .map_err(|e| KernelError::Config(format!("locale {locale} 解析失败: {e}")))?,
            );
        }
        Ok(Self {
            settings,
            base,
            module_packs: RwLock::new(HashMap::new()),
        })
    }

    /// 支持的基准语言列表。
    pub fn supported_locales(&self) -> Vec<String> {
        let mut list: Vec<String> = self.base.keys().cloned().collect();
        list.sort();
        list
    }

    /// 当前语言（设置持久化，非法值回退 en-US）。
    pub fn current_locale(&self) -> String {
        let locale = self.settings.get_or("locale", FALLBACK_LOCALE.to_string());
        if self.base.contains_key(&locale) {
            locale
        } else {
            FALLBACK_LOCALE.to_string()
        }
    }

    /// 切换语言并持久化。
    pub fn set_locale(&self, locale: &str) -> Result<(), KernelError> {
        if !self.base.contains_key(locale) {
            return Err(KernelError::InvalidArgument(format!(
                "unsupported locale: {locale}"
            )));
        }
        self.settings.set("locale", &locale)
    }

    /// 注册模块语言包（模块在 `init` 阶段调用）。
    pub fn register_module_pack(
        &self,
        module_id: &str,
        locale: &str,
        pack: Value,
    ) -> Result<(), KernelError> {
        if !pack.is_object() {
            return Err(KernelError::InvalidArgument(
                "module language pack must be an object".into(),
            ));
        }
        self.module_packs
            .write()
            .insert((module_id.to_string(), locale.to_string()), pack);
        Ok(())
    }

    /// 取某语言完整目录（基准 + 模块语言包），供前端一次性加载。
    ///
    /// 模块语言包优先取目标语言，缺失时才回退 en-US，避免回退包覆盖目标包。
    pub fn catalog(&self, locale: &str) -> Value {
        let mut merged = self
            .base
            .get(locale)
            .cloned()
            .unwrap_or_else(|| self.base[FALLBACK_LOCALE].clone());
        let packs = self.module_packs.read().clone();

        let mut module_root = serde_json::Map::new();
        // 第一遍：目标语言包（en-US 即目标语言时也在此覆盖）。
        for ((module, loc), pack) in &packs {
            if loc == locale {
                // 模块内容挂到 `module.<名>` 命名空间下。
                module_root.insert(module.clone(), pack.clone());
            }
        }
        // 第二遍：仅补缺失模块的 en-US 回退包，不回退已有目标包。
        if locale != FALLBACK_LOCALE {
            for ((module, loc), pack) in &packs {
                if loc == FALLBACK_LOCALE && !module_root.contains_key(module) {
                    module_root.insert(module.clone(), pack.clone());
                }
            }
        }

        if !module_root.is_empty() {
            if let Some(obj) = merged.as_object_mut() {
                obj.insert("module".to_string(), Value::Object(module_root));
            }
        }
        merged
    }

    /// 取单个文案（点分路径）。回退链：locale → en-US → 键名。
    pub fn t(&self, locale: &str, key: &str) -> String {
        let catalog = self.catalog(locale);
        lookup(&catalog, key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                let en = self.base.get(FALLBACK_LOCALE).cloned().unwrap_or_default();
                lookup(&en, key)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| key.to_string())
            })
    }
}

/// 点分路径查询嵌套 JSON。
fn lookup<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = root;
    for part in key.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}
