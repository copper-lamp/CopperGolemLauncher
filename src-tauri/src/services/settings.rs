//! 设置服务：基于 `core_settings` KV 表的类型化读写。
//!
//! - 值以 JSON 存储；读写走内存缓存，写后同步落库。
//! - 任何写操作广播 `settings.changed` 事件（负载为发生变更的键值集合），模块据此联动。
//! - 默认值在 [`defaults`] 集中定义（对应设置页 通用 / 启动 / 个性 / 模块 四类）。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use crate::error::KernelError;
use crate::registry::events::EventBus;
use crate::services::database::DatabaseService;

/// 设置服务。
pub struct SettingsService {
    db: Arc<DatabaseService>,
    events: Arc<EventBus>,
    cache: RwLock<HashMap<String, Value>>,
    defaults: HashMap<String, Value>,
}

impl SettingsService {
    /// 创建设置服务并载入数据库缓存。`defaults` 为扁平化默认键值（点分键）。
    pub fn new(
        db: Arc<DatabaseService>,
        events: Arc<EventBus>,
        defaults: HashMap<String, Value>,
    ) -> Result<Self, KernelError> {
        let service = Self {
            db,
            events,
            cache: RwLock::new(HashMap::new()),
            defaults,
        };
        service.reload()?;
        Ok(service)
    }

    /// 从数据库全量重载缓存。
    pub fn reload(&self) -> Result<(), KernelError> {
        let mut cache = self.cache.write();
        cache.clear();
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT key, value FROM core_settings")?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (key, raw) = row?;
                let value: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
                cache.insert(key, value);
            }
            Ok(())
        })?;
        Ok(())
    }

    /// 读取（先缓存后默认，不存在返回 None）。
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        if let Some(v) = self.cache.read().get(key) {
            if !v.is_null() {
                return serde_json::from_value(v.clone()).ok();
            }
        }
        self.defaults
            .get(key)
            .and_then(|d| serde_json::from_value(d.clone()).ok())
    }

    /// 读取，带默认值兜底。
    pub fn get_or<T: DeserializeOwned + Clone>(&self, key: &str, default: T) -> T {
        self.get::<T>(key).unwrap_or(default)
    }

    /// 写入单个键并广播变更事件。
    pub fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<(), KernelError> {
        let value = serde_json::to_value(value)?;
        self.cache.write().insert(key.to_string(), value.clone());
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO core_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![key, value.to_string()],
            )?;
            Ok(())
        })?;
        let mut changed = HashMap::new();
        changed.insert(key.to_string(), value);
        self.events.publish("settings.changed", Value::Object(changed.into_iter().collect()));
        Ok(())
    }

    /// 批量写入并广播一次变更事件。
    pub fn set_many(
        &self,
        entries: &HashMap<String, Value>,
    ) -> Result<(), KernelError> {
        let mut changed = HashMap::new();
        self.db.with_conn(|conn| {
            for (key, value) in entries {
                conn.execute(
                    "INSERT INTO core_settings (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    rusqlite::params![key, value.to_string()],
                )?;
                changed.insert(key.clone(), value.clone());
            }
            Ok(())
        })?;
        {
            let mut cache = self.cache.write();
            cache.extend(changed.clone());
        }
        self.events.publish("settings.changed", Value::Object(changed.into_iter().collect()));
        Ok(())
    }

    /// 全量设置快照（含默认值）。
    pub fn all(&self) -> HashMap<String, Value> {
        let mut map = self.defaults.clone();
        for (k, v) in self.cache.read().iter() {
            map.insert(k.clone(), v.clone());
        }
        map
    }
}

/// 默认设置（扁平点分键）。
pub fn defaults() -> HashMap<String, Value> {
    let mut m = HashMap::new();
    // 通用
    m.insert("locale".into(), Value::String("zh-CN".into()));
    m.insert("theme.mode".into(), Value::String("dark".into()));
    m.insert("theme.accent".into(), Value::String("#c97b3d".into()));
    // 游戏目录（版本根；空 = 使用默认 %APPDATA%/.../versions）
    m.insert("game.directory".into(), Value::String(String::new()));
    // LLM 模型表（非敏感项）由 `services::llm` 以单键 `llm.models`（JSON 数组）自持，
    // 缺失即空表，故不在此设默认。API Key 不走设置——密钥环存储见 `services::llm`
    // （settings_all 会整表下发前端）。内核不预设任何 base URL。
    // 启动
    m.insert("launch.default_version".into(), Value::String(String::new()));
    m.insert("launch.memory_mb".into(), Value::Number(4096.into()));
    m.insert("launch.args".into(), Value::String(String::new()));
    m.insert("launch.show_logs".into(), Value::Bool(false));
    m.insert("launch.after_launch".into(), Value::String("keep".into()));
    // 个性
    m.insert("appearance.list_density".into(), Value::String("comfortable".into()));
    m.insert("appearance.animations".into(), Value::Bool(true));
    m.insert("performance.render".into(), Value::String("balanced".into()));
    // 下载：资产层镜像偏好（auto / github / jsdelivr / ghproxy / gitcode）。
    // 只影响**资产层**（安装包、图标）的地址排序；索引层固定走内置三段镜像，
    // 不消费本设置——索引是所有内容的信任锚，不能被用户配置的第三方镜像替换
    // （见 docs/cgl-libs.md 2.6）。
    m.insert("download.mirror".into(), Value::String("auto".into()));
    // 下载：同时下载数（1~5）。运行期可即时调整，见 `commands::download`。
    m.insert("download.concurrency".into(), Value::Number(3.into()));
    // 下载：下载页「最近下载」展示条目上限（0 = 不显示）。
    // 历史记录仅存在于内存，故这里限制的是**展示窗口**而非持久化策略。
    m.insert("download.history_limit".into(), Value::Number(50.into()));
    // 模块：元数据发布通道过滤（stable / beta / dev，可用 `+` 组合，如 `stable+beta`）。
    m.insert("registry.channel".into(), Value::String("stable".into()));
    // 内容下载：列表筛选持久化（来源 / 类型 / 游戏版本 / 排序方式；空串 = 不限）。
    m.insert("content.filter.source".into(), Value::String(String::new()));
    m.insert("content.filter.type".into(), Value::String(String::new()));
    m.insert("content.filter.version".into(), Value::String(String::new()));
    m.insert(
        "content.filter.sort".into(),
        Value::String("downloads_desc".into()),
    );
    m
}
