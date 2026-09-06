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
    m
}
