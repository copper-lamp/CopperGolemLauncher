//! LLM 模型表服务：内核内任何 AI 相关服务的**单一访问器**。
//!
//! 内核极简——只保存用户维护的「模型表」：每条含 base URL、模型名称、可选显示名称。
//! **不内置** provider 表、预设 base URL、API 方言或 contextWindow / maxTokens；
//! baseUrl 与 provider 的匹配属 cgl-agent 模块职责，本服务不感知。
//! 内核也**不标记**哪一条是「默认 / 当前」模型：调用方按 `id` 指定要用的条目。
//!
//! 存储分工（安全边界）：
//! - 非敏感项走设置服务，单一键 `llm.models`，值为 JSON 数组；
//! - 每条模型的密钥走系统密钥环（[`SecretStore`]），账户名 `llm.api_key:{id}`，
//!   **绝不进设置**——`settings_all` 会把整表（含值）下发给前端，密钥入库会造成
//!   双重回显。密钥环 service 复用账户服务的 `copper-golem` 命名空间。
//!
//! 显示名称规则：`display_name` 存用户填写的**原始值**（可空，不落库成计算后的
//! 结果），有效显示名在读取时计算——trim 后非空则用它，否则回退为 `model` 的
//! trim 值（[`LlmModel::effective_name`]）。有效显示名在表内必须**唯一**（比较前
//! trim，区分大小写），冲突由本服务拒绝并回可操作错误。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::platform::secret::SharedSecretStore;
use crate::services::settings::SettingsService;

/// 模型表的设置键；值为 JSON 数组。
pub const LLM_MODELS_KEY: &str = "llm.models";

/// 密钥环命名空间（与 `services::account` 保持一致）。
const KEYRING_SERVICE: &str = "copper-golem";

/// 单条模型对应的密钥环账户名。
fn keyring_account(id: &str) -> String {
    format!("llm.api_key:{id}")
}

/// 单条 LLM 模型（**不含密钥**）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmModel {
    pub id: String,
    /// 用户填写的显示名称；存原始值，可空。有效名见 [`LlmModel::effective_name`]。
    #[serde(default)]
    pub display_name: String,
    pub base_url: String,
    pub model: String,
}

impl LlmModel {
    /// 有效显示名：`display_name` trim 后非空则用它，否则回退为 `model` 的 trim 值。
    pub fn effective_name(&self) -> &str {
        let name = self.display_name.trim();
        if name.is_empty() {
            self.model.trim()
        } else {
            name
        }
    }
}

/// 供前端表格使用的模型行（**不含密钥**，仅标注是否已配置）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LlmModelRow {
    pub id: String,
    pub display_name: String,
    pub effective_name: String,
    pub base_url: String,
    pub model: String,
    pub api_key_configured: bool,
}

/// LLM 模型表服务。
pub struct LlmConfigService {
    settings: Arc<SettingsService>,
    secret: SharedSecretStore,
}

impl LlmConfigService {
    pub fn new(settings: Arc<SettingsService>, secret: SharedSecretStore) -> Self {
        Self { settings, secret }
    }

    /// 读取整张模型表（按存储顺序；缺失即空表，不报错）。
    pub fn list(&self) -> Vec<LlmModel> {
        self.settings
            .get_or::<Vec<LlmModel>>(LLM_MODELS_KEY, Vec::new())
    }

    /// 模型表 + 每条的密钥配置状态（给前端表格）。
    pub fn rows(&self) -> Vec<LlmModelRow> {
        self.list()
            .into_iter()
            .map(|m| {
                let effective_name = m.effective_name().to_string();
                let api_key_configured = self.api_key_configured(&m.id);
                LlmModelRow {
                    id: m.id,
                    display_name: m.display_name,
                    effective_name,
                    base_url: m.base_url,
                    model: m.model,
                    api_key_configured,
                }
            })
            .collect()
    }

    /// 按 id 取单条（不存在返回 `None`）。
    pub fn get(&self, id: &str) -> Option<LlmModel> {
        self.list().into_iter().find(|m| m.id == id)
    }

    /// 新增一条模型，返回新生成的 id。
    ///
    /// 校验：base URL 与模型名称 trim 后不得为空；有效显示名不得与既有条目重复。
    pub fn add(
        &self,
        display_name: String,
        base_url: String,
        model: String,
    ) -> Result<String, KernelError> {
        let mut models = self.list();
        let entry = LlmModel {
            id: uuid::Uuid::new_v4().to_string(),
            display_name,
            base_url: base_url.trim().to_string(),
            model: model.trim().to_string(),
        };
        validate_required(&entry)?;
        let effective = entry.effective_name().to_string();
        if let Some(conflict) = conflicting_name(&models, &effective, None) {
            return Err(name_conflict_error(&conflict));
        }
        let id = entry.id.clone();
        models.push(entry);
        self.save(&models)?;
        Ok(id)
    }

    /// 按 id 更新一条模型。
    ///
    /// 校验同 [`Self::add`]；唯一性检查**排除自身**，否则「保持原名再保存」会被
    /// 误判为冲突。id 不存在返回明确错误。
    pub fn update(
        &self,
        id: &str,
        display_name: String,
        base_url: String,
        model: String,
    ) -> Result<(), KernelError> {
        let mut models = self.list();
        let index = models
            .iter()
            .position(|m| m.id == id)
            .ok_or_else(|| KernelError::InvalidArgument(format!("模型不存在: {id}")))?;
        let candidate = LlmModel {
            id: id.to_string(),
            display_name,
            base_url: base_url.trim().to_string(),
            model: model.trim().to_string(),
        };
        validate_required(&candidate)?;
        let effective = candidate.effective_name().to_string();
        if let Some(conflict) = conflicting_name(&models, &effective, Some(id)) {
            return Err(name_conflict_error(&conflict));
        }
        models[index] = candidate;
        self.save(&models)
    }

    /// 按 id 删除一条模型，并清除其密钥。
    ///
    /// **幂等**：`id` 不在表中时同样返回 `Ok(())`（与 `SecretStore::delete` 的
    /// 「不存在视为成功」语义一致，前端可能持有陈旧列表）。但无论条目是否在表中，
    /// 都会清一次该 id 的密钥，避免残留孤儿凭证。
    pub fn remove(&self, id: &str) -> Result<(), KernelError> {
        let mut models = self.list();
        let before = models.len();
        models.retain(|m| m.id != id);
        if models.len() != before {
            self.save(&models)?;
        }
        self.secret
            .delete(KEYRING_SERVICE, &keyring_account(id))
            .map_err(KernelError::Secret)
    }

    /// 写入某条模型的密钥；`id` 必须存在，trim 后为空报参数错误。
    ///
    /// **不写任何日志**，避免密钥经日志外泄。
    pub fn set_api_key(&self, id: &str, key: &str) -> Result<(), KernelError> {
        if self.get(id).is_none() {
            return Err(KernelError::InvalidArgument(format!("模型不存在: {id}")));
        }
        let key = key.trim();
        if key.is_empty() {
            return Err(KernelError::InvalidArgument("API Key 不能为空".into()));
        }
        self.secret
            .set(KEYRING_SERVICE, &keyring_account(id), key)
            .map_err(KernelError::Secret)
    }

    /// 清除某条模型的密钥；`id` 必须存在（幂等——密钥本就不存在也返回 `Ok`）。
    pub fn clear_api_key(&self, id: &str) -> Result<(), KernelError> {
        if self.get(id).is_none() {
            return Err(KernelError::InvalidArgument(format!("模型不存在: {id}")));
        }
        self.secret
            .delete(KEYRING_SERVICE, &keyring_account(id))
            .map_err(KernelError::Secret)
    }

    /// 读取某条模型的密钥；不存在或为空串均视为未配置。
    ///
    /// 不校验 id 是否仍在表中：密钥环按 id 寻址，孤儿条目同样可读（便于清理）。
    pub fn api_key(&self, id: &str) -> Result<Option<String>, KernelError> {
        let key = self
            .secret
            .get(KEYRING_SERVICE, &keyring_account(id))
            .map_err(KernelError::Secret)?;
        Ok(key.filter(|k| !k.trim().is_empty()))
    }

    /// 某条模型是否已配置密钥。
    pub fn api_key_configured(&self, id: &str) -> bool {
        matches!(self.api_key(id), Ok(Some(_)))
    }

    /// 组合取用：按 id 取「配置 + 密钥」，供后续下发 Agent 用。
    ///
    /// 条目不存在返回 `None`；密钥缺失时密钥为 `None`。本方法返回 `Option`，
    /// 无法透出密钥环读取错误——需要区分「读取失败」与「未配置」时请直接调用
    /// [`Self::api_key`]。
    pub fn resolve(&self, id: &str) -> Option<(LlmModel, Option<String>)> {
        let model = self.get(id)?;
        let key = self.api_key(id).ok().flatten();
        Some((model, key))
    }

    /// 落库整张模型表（单一键，值即数组）。
    fn save(&self, models: &[LlmModel]) -> Result<(), KernelError> {
        self.settings.set(LLM_MODELS_KEY, &models)
    }
}

/// 必填校验：base URL 与模型名称 trim 后不得为空（构造时已 trim，故判空即可）。
fn validate_required(model: &LlmModel) -> Result<(), KernelError> {
    if model.base_url.is_empty() {
        return Err(KernelError::InvalidArgument("接口地址不能为空".into()));
    }
    if model.model.is_empty() {
        return Err(KernelError::InvalidArgument("模型名称不能为空".into()));
    }
    Ok(())
}

/// 查找与 `name` 冲突（有效显示名相同）的既有条目，`exclude_id` 用于排除自身。
fn conflicting_name(models: &[LlmModel], name: &str, exclude_id: Option<&str>) -> Option<String> {
    models
        .iter()
        .find(|m| exclude_id != Some(m.id.as_str()) && m.effective_name() == name)
        .map(|m| m.effective_name().to_string())
}

/// 冲突错误：消息带上冲突的有效显示名，便于用户直接定位。
fn name_conflict_error(name: &str) -> KernelError {
    KernelError::Conflict(format!("显示名称「{name}」已存在，请修改显示名称或模型名称"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use parking_lot::Mutex;
    use serde_json::Value;

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
        let root = std::env::temp_dir().join(format!("copper-llm-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).expect("创建测试目录");
        let db = Arc::new(DatabaseService::open(&root.join("core.db")).expect("打开测试数据库"));
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

    /// 便捷新增：返回 id。
    fn add(f: &Fixture, display_name: &str, base_url: &str, model: &str) -> String {
        f.llm
            .add(display_name.into(), base_url.into(), model.into())
            .expect("新增模型应成功")
    }

    #[test]
    fn empty_table_reads_empty() {
        let f = fixture();
        assert!(f.llm.list().is_empty());
        assert!(f.llm.rows().is_empty());
        assert!(f.llm.get("missing").is_none());
    }

    #[test]
    fn add_then_list_and_get_roundtrip() {
        let f = fixture();
        let a = add(&f, "主模型", " https://a.example/v1 ", " gpt-4o-mini ");
        let b = add(&f, "", "https://b.example/v1", "demo-model");

        // 顺序即存储顺序；base_url / model 落库前 trim，display_name 存原始值。
        let list = f.llm.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, a);
        assert_eq!(list[0].display_name, "主模型");
        assert_eq!(list[0].base_url, "https://a.example/v1");
        assert_eq!(list[0].model, "gpt-4o-mini");
        assert_eq!(list[1].id, b);
        assert_eq!(list[1].display_name, "");
        assert_eq!(list[1].effective_name(), "demo-model");

        // 返回的 id 可被 get 取到。
        let got = f.llm.get(&a).expect("按 id 取回");
        assert_eq!(got.model, "gpt-4o-mini");
        assert_eq!(f.llm.get(&b).map(|m| m.base_url), Some("https://b.example/v1".into()));
    }

    #[test]
    fn effective_name_falls_back_to_model() {
        let f = fixture();
        // 显示名全空白 → 回退模型名（trim）。
        let id = add(&f, "   ", "https://a.example/v1", "  my-model  ");
        let row = f.llm.rows().into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.display_name, "   ");
        assert_eq!(row.effective_name, "my-model");

        // 显示名非空 → 用显示名（trim）。
        let id = add(&f, "  别名  ", "https://b.example/v1", "other-model");
        let row = f.llm.rows().into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.effective_name, "别名");
    }

    #[test]
    fn rows_report_key_state_without_exposing_key() {
        let f = fixture();
        let a = add(&f, "甲", "https://a.example/v1", "m-a");
        let b = add(&f, "乙", "https://b.example/v1", "m-b");
        f.llm.set_api_key(&a, "sk-secret").unwrap();

        let rows = f.llm.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].api_key_configured);
        assert!(!rows[1].api_key_configured);
        assert_eq!(rows[0].effective_name, "甲");
        assert_eq!(rows[1].id, b);
    }

    #[test]
    fn required_fields_are_rejected_without_writing() {
        let f = fixture();
        for (base_url, model) in [("", "m"), ("   ", "m"), ("https://a.example/v1", ""), ("https://a.example/v1", "  ")] {
            let err = f
                .llm
                .add("n".into(), base_url.into(), model.into())
                .unwrap_err();
            assert!(
                matches!(err, KernelError::InvalidArgument(_)),
                "必填缺失应报参数错误，实得 {err}"
            );
        }
        assert!(f.llm.list().is_empty(), "校验失败不得写入");

        // update 同样校验必填。
        let id = add(&f, "甲", "https://a.example/v1", "m-a");
        let err = f
            .llm
            .update(&id, "甲".into(), "".into(), "m-a".into())
            .unwrap_err();
        assert!(matches!(err, KernelError::InvalidArgument(_)));
        assert_eq!(f.llm.get(&id).unwrap().base_url, "https://a.example/v1");
    }

    #[test]
    fn duplicate_effective_names_are_rejected() {
        let f = fixture();

        // 1) 显示名相同 → 拒绝。
        add(&f, "同名", "https://a.example/v1", "m-a");
        let err = f
            .llm
            .add(" 同名 ".into(), "https://b.example/v1".into(), "m-b".into())
            .unwrap_err();
        assert!(matches!(err, KernelError::Conflict(_)), "实得 {err}");
        assert!(err.to_string().contains("同名"), "错误应带上冲突名: {err}");
        assert_eq!(f.llm.list().len(), 1, "冲突不得写入");

        // 2) 两条都留空显示名、模型名相同（回退后同名）→ 拒绝。
        let f2 = fixture();
        add(&f2, "", "https://a.example/v1", "same-model");
        let err = f2
            .llm
            .add("".into(), "https://b.example/v1".into(), " same-model ".into())
            .unwrap_err();
        assert!(matches!(err, KernelError::Conflict(_)), "实得 {err}");
        assert!(err.to_string().contains("same-model"), "错误应带上冲突名: {err}");

        // 3) update 把 A 改成与 B 相同 → 拒绝。
        let f3 = fixture();
        let a = add(&f3, "甲", "https://a.example/v1", "m-a");
        add(&f3, "乙", "https://b.example/v1", "m-b");
        let err = f3
            .llm
            .update(&a, "乙".into(), "https://a.example/v1".into(), "m-a".into())
            .unwrap_err();
        assert!(matches!(err, KernelError::Conflict(_)), "实得 {err}");
        assert_eq!(f3.llm.get(&a).unwrap().display_name, "甲", "冲突不得写入");

        // 4) update 保持自己原名 → 允许（唯一性检查排除自身）。
        f3.llm
            .update(&a, "甲".into(), "https://a2.example/v1".into(), "m-a2".into())
            .unwrap();
        let updated = f3.llm.get(&a).unwrap();
        assert_eq!(updated.display_name, "甲");
        assert_eq!(updated.base_url, "https://a2.example/v1");
    }

    #[test]
    fn update_and_key_ops_require_existing_id() {
        let f = fixture();
        let err = f
            .llm
            .update("nope", "n".into(), "https://a.example/v1".into(), "m".into())
            .unwrap_err();
        assert!(matches!(err, KernelError::InvalidArgument(_)), "实得 {err}");
        assert!(err.to_string().contains("nope"));

        assert!(matches!(
            f.llm.set_api_key("nope", "sk-x").unwrap_err(),
            KernelError::InvalidArgument(_)
        ));
        assert!(matches!(
            f.llm.clear_api_key("nope").unwrap_err(),
            KernelError::InvalidArgument(_)
        ));
        assert_eq!(f.store.len(), 0, "不存在的 id 不得写入密钥环");
    }

    #[test]
    fn api_key_lifecycle_per_model() {
        let f = fixture();
        let a = add(&f, "甲", "https://a.example/v1", "m-a");
        let b = add(&f, "乙", "https://b.example/v1", "m-b");

        f.llm.set_api_key(&a, "  sk-test-key  ").unwrap();
        assert_eq!(f.llm.api_key(&a).unwrap().as_deref(), Some("sk-test-key"));
        assert!(f.llm.api_key_configured(&a));
        assert!(!f.llm.api_key_configured(&b));
        assert!(
            f.llm.rows().iter().find(|r| r.id == a).unwrap().api_key_configured
        );

        // resolve：配置 + 密钥组合取用。
        let (model, key) = f.llm.resolve(&a).unwrap();
        assert_eq!(model.model, "m-a");
        assert_eq!(key.as_deref(), Some("sk-test-key"));
        assert!(f.llm.resolve("nope").is_none());

        // clear 后回 false。
        f.llm.clear_api_key(&a).unwrap();
        assert_eq!(f.llm.api_key(&a).unwrap(), None);
        assert!(!f.llm.api_key_configured(&a));
        assert!(!f.llm.rows().iter().find(|r| r.id == a).unwrap().api_key_configured);

        // remove 同时清掉该 id 的密钥。
        f.llm.set_api_key(&b, "sk-b").unwrap();
        assert_eq!(f.store.len(), 1);
        f.llm.remove(&b).unwrap();
        assert_eq!(f.store.len(), 0, "删除模型应清掉其密钥");
        assert!(f.llm.get(&b).is_none());
        assert_eq!(f.llm.list().len(), 1);
    }

    #[test]
    fn remove_is_idempotent() {
        let f = fixture();
        // 不存在的 id：返回 Ok，且仍清一次（可能残留的）密钥。
        f.llm.set_api_key("ghost", "sk-ghost").ok();
        f.store
            .set(KEYRING_SERVICE, &keyring_account("ghost"), "sk-orphan")
            .unwrap();
        assert_eq!(f.store.len(), 1);
        f.llm.remove("ghost").unwrap();
        assert_eq!(f.store.len(), 0, "幂等删除也应清掉孤儿密钥");
    }

    #[test]
    fn blank_api_key_is_rejected_without_writing() {
        let f = fixture();
        let id = add(&f, "甲", "https://a.example/v1", "m-a");
        let err = f.llm.set_api_key(&id, "   ").unwrap_err();
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
        let id = add(&f, "甲", "https://a.example/v1", "m-a");
        f.llm.set_api_key(&id, KEY).unwrap();

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

        // `llm.models` 里不得出现密钥，也不得出现密钥字段名。
        let raw = f
            .settings
            .get::<Value>(LLM_MODELS_KEY)
            .expect("模型表应已落库");
        let text = raw.to_string();
        assert!(!text.contains(KEY), "模型表泄露了密钥: {text}");
        assert!(!text.contains("api_key"), "模型表出现了密钥字段: {text}");
    }

    #[test]
    fn errors_never_expose_the_key() {
        let f = fixture();
        const KEY: &str = "sk-error-canary-4321";
        let id = add(&f, "甲", "https://a.example/v1", "m-a");
        f.store.set_failing(true);
        let err = f.llm.set_api_key(&id, KEY).unwrap_err();
        assert!(
            !err.to_string().contains(KEY),
            "错误信息泄露了密钥: {err}"
        );

        f.store.set_failing(false);
        let err = f.llm.set_api_key(&id, "   ").unwrap_err();
        assert!(matches!(err, KernelError::InvalidArgument(_)));
        assert!(
            !err.to_string().contains("sk-error-canary"),
            "参数错误回显了输入: {err}"
        );
    }
}
