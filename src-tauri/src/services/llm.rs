//! LLM 配置服务：内核内任何 AI 相关服务的**单一访问器**。
//!
//! 内核极简——只保存用户填的三项：base URL、模型名称、API Key。**不内置** provider
//! 表、预设 base URL、API 方言或 contextWindow / maxTokens；baseUrl 与 provider 的
//! 匹配属 cgl-agent 模块职责，本服务不感知。
//!
//! 存储分工（安全边界）：
//! - 非敏感项（`llm.base_url` / `llm.model`）走设置服务；
//! - 密钥走系统密钥环（[`SecretStore`]），**绝不进设置**——`settings_all` 会把整表
//!   （含值）下发给前端，密钥入库会造成双重回显。密钥环账户复用账户服务的
//!   `copper-golem` 命名空间，账户名 `llm.api_key`。

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::KernelError;
use crate::platform::secret::SharedSecretStore;
use crate::services::settings::SettingsService;

/// 非敏感设置键：base URL。
pub const LLM_BASE_URL_KEY: &str = "llm.base_url";
/// 非敏感设置键：模型名称。
pub const LLM_MODEL_KEY: &str = "llm.model";
/// 密钥环账户名（service 复用账户服务的 `copper-golem`）。
pub const LLM_API_KEY_ACCOUNT: &str = "llm.api_key";

/// 密钥环命名空间（与 `services::account` 保持一致）。
const KEYRING_SERVICE: &str = "copper-golem";

/// LLM 非敏感配置（**不含密钥**）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
}

/// LLM 配置服务。
pub struct LlmConfigService {
    settings: Arc<SettingsService>,
    secret: SharedSecretStore,
}

impl LlmConfigService {
    pub fn new(settings: Arc<SettingsService>, secret: SharedSecretStore) -> Self {
        Self { settings, secret }
    }

    /// 读取非敏感配置（缺失即空串，不报错）。
    pub fn config(&self) -> LlmConfig {
        LlmConfig {
            base_url: self.settings.get_or(LLM_BASE_URL_KEY, String::new()),
            model: self.settings.get_or(LLM_MODEL_KEY, String::new()),
        }
    }

    /// 读取密钥；不存在或为空串均视为未配置。
    pub fn api_key(&self) -> Result<Option<String>, KernelError> {
        let key = self
            .secret
            .get(KEYRING_SERVICE, LLM_API_KEY_ACCOUNT)
            .map_err(KernelError::Secret)?;
        Ok(key.filter(|k| !k.trim().is_empty()))
    }

    /// 是否已配置密钥。
    pub fn api_key_configured(&self) -> bool {
        matches!(self.api_key(), Ok(Some(_)))
    }

    /// 写入密钥（trim 后为空报参数错误）。**不写任何日志**，避免密钥经日志外泄。
    pub fn set_api_key(&self, key: &str) -> Result<(), KernelError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(KernelError::InvalidArgument("API Key 不能为空".into()));
        }
        self.secret
            .set(KEYRING_SERVICE, LLM_API_KEY_ACCOUNT, key)
            .map_err(KernelError::Secret)
    }

    /// 清除密钥（幂等）。
    pub fn clear_api_key(&self) -> Result<(), KernelError> {
        self.secret
            .delete(KEYRING_SERVICE, LLM_API_KEY_ACCOUNT)
            .map_err(KernelError::Secret)
    }

    /// 保存非敏感配置（两项 trim 后写设置；允许为空串，表示未配置）。
    pub fn save_config(&self, config: &LlmConfig) -> Result<(), KernelError> {
        let mut entries = HashMap::new();
        entries.insert(
            LLM_BASE_URL_KEY.to_string(),
            Value::String(config.base_url.trim().to_string()),
        );
        entries.insert(
            LLM_MODEL_KEY.to_string(),
            Value::String(config.model.trim().to_string()),
        );
        self.settings.set_many(&entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use parking_lot::Mutex;

    use crate::platform::secret::SecretStore;
    use crate::registry::events::EventBus;
    use crate::services::database::{DatabaseService, CORE_MIGRATIONS};
    use crate::services::settings::defaults;

    /// 内存桩密钥环：验证读写删契约，不触碰真实系统密钥环。可切换为失败模式，
    /// 用于验证错误信息不含密钥。
    struct StubSecretStore {
        entries: Mutex<HashMap<String, String>>,
        failing: AtomicBool,
    }

    impl StubSecretStore {
        fn new() -> Self {
            Self {
                entries: Mutex::new(HashMap::new()),
                failing: AtomicBool::new(false),
            }
        }

        fn set_failing(&self, failing: bool) {
            self.failing.store(failing, Ordering::Relaxed);
        }

        fn len(&self) -> usize {
            self.entries.lock().len()
        }

        fn key(service: &str, account: &str) -> String {
            format!("{service}/{account}")
        }
    }

    impl SecretStore for StubSecretStore {
        fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String> {
            if self.failing.load(Ordering::Relaxed) {
                return Err("密钥环不可用".to_string());
            }
            self.entries
                .lock()
                .insert(Self::key(service, account), value.to_string());
            Ok(())
        }

        fn get(&self, service: &str, account: &str) -> Result<Option<String>, String> {
            Ok(self.entries.lock().get(&Self::key(service, account)).cloned())
        }

        fn delete(&self, service: &str, account: &str) -> Result<(), String> {
            self.entries.lock().remove(&Self::key(service, account));
            Ok(())
        }
    }

    /// 测试夹具：真实 `SettingsService`（独立临时库）+ 桩密钥环。
    struct Fixture {
        llm: LlmConfigService,
        settings: Arc<SettingsService>,
        store: Arc<StubSecretStore>,
    }

    fn fixture() -> Fixture {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "copper-llm-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("创建测试目录");
        let db = Arc::new(
            DatabaseService::open(&root.join("core.db")).expect("打开测试数据库"),
        );
        db.migrate_scope("core", CORE_MIGRATIONS)
            .expect("迁移内核 schema");
        let settings = Arc::new(
            SettingsService::new(db, Arc::new(EventBus::new()), defaults())
                .expect("装载设置服务"),
        );
        let store = Arc::new(StubSecretStore::new());
        let secret: SharedSecretStore = store.clone();
        let llm = LlmConfigService::new(settings.clone(), secret);
        Fixture {
            llm,
            settings,
            store,
        }
    }

    #[test]
    fn unconfigured_reads_empty_and_not_configured() {
        let f = fixture();
        let config = f.llm.config();
        assert_eq!(config.base_url, "");
        assert_eq!(config.model, "");
        assert!(!f.llm.api_key_configured());
        assert_eq!(f.llm.api_key().unwrap(), None);
    }

    #[test]
    fn save_and_clear_roundtrip() {
        let f = fixture();
        f.llm
            .save_config(&LlmConfig {
                base_url: "  https://example.com/v1  ".into(),
                model: "  demo-model  ".into(),
            })
            .unwrap();
        let config = f.llm.config();
        assert_eq!(config.base_url, "https://example.com/v1");
        assert_eq!(config.model, "demo-model");

        f.llm.set_api_key("sk-test-key").unwrap();
        assert_eq!(f.llm.api_key().unwrap().as_deref(), Some("sk-test-key"));
        assert!(f.llm.api_key_configured());

        f.llm.clear_api_key().unwrap();
        assert_eq!(f.llm.api_key().unwrap(), None);
        assert!(!f.llm.api_key_configured());
    }

    #[test]
    fn blank_api_key_is_rejected_without_writing() {
        let f = fixture();
        let err = f.llm.set_api_key("   ").unwrap_err();
        assert!(
            matches!(err, KernelError::InvalidArgument(_)),
            "空密钥应报参数错误，实得 {err}"
        );
        assert_eq!(f.store.len(), 0, "空密钥不得写入密钥环");
    }

    /// 回归护栏（本批最关键的安全断言）：密钥绝不进入设置表，否则
    /// `settings_all` 会把密钥整表下发前端。
    #[test]
    fn api_key_never_leaches_into_settings() {
        let f = fixture();
        const KEY: &str = "sk-leak-canary-9876";
        f.llm.set_api_key(KEY).unwrap();

        let dumped: Vec<String> = f
            .settings
            .all()
            .values()
            .map(|v| v.to_string())
            .collect();
        assert!(
            dumped.iter().all(|s| !s.contains(KEY)),
            "settings 快照泄露了密钥: {dumped:?}"
        );
    }

    #[test]
    fn errors_never_expose_the_key() {
        let f = fixture();
        const KEY: &str = "sk-error-canary-4321";
        f.store.set_failing(true);
        let err = f.llm.set_api_key(KEY).unwrap_err();
        assert!(
            !err.to_string().contains(KEY),
            "错误信息泄露了密钥: {err}"
        );

        let err = f.llm.set_api_key("   ").unwrap_err();
        assert!(matches!(err, KernelError::InvalidArgument(_)));
        assert!(
            !err.to_string().contains("sk-error-canary"),
            "参数错误回显了输入: {err}"
        );
    }
}
