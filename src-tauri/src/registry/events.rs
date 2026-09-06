//! 事件总线：模块间广播式联动中介。
//!
//! 语义：
//! - 发布 / 订阅；订阅可带通配后缀（`download.*` 匹配 `download.created` 等）。
//! - **至少一次**：未就绪订阅者错过的事件缓存在有界重放缓冲，订阅时补发。
//! - 内核发布的同名事件自动桥接到前端（Tauri `emit`），前端可 `listen` 同名事件。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

const REPLAY_CAP: usize = 64;

type Handler = Arc<dyn Fn(&str, &Value) + Send + Sync>;

struct Subscriber {
    id: u64,
    pattern: String,
    handler: Handler,
}

/// 订阅句柄，模块持有它以便在 stop 时退订。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subscription(pub u64);

/// 事件总线。
pub struct EventBus {
    subscribers: RwLock<HashMap<String, Vec<Subscriber>>>,
    replay: Mutex<VecDeque<(String, Value)>>,
    next_id: AtomicU64,
    app: Mutex<Option<AppHandle>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            subscribers: RwLock::new(HashMap::new()),
            replay: Mutex::new(VecDeque::new()),
            next_id: AtomicU64::new(1),
            app: Mutex::new(None),
        }
    }

    /// 绑定 AppHandle（setup 后调用），用于向前端桥接事件。
    pub fn bind_app(&self, app: AppHandle) {
        *self.app.lock() = Some(app);
    }

    /// 订阅事件。已匹配的重放缓冲事件会立即补发。
    pub fn subscribe(
        &self,
        pattern: impl Into<String>,
        handler: impl Fn(&str, &Value) + Send + Sync + 'static,
    ) -> Subscription {
        let pattern = pattern.into();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let handler: Handler = Arc::new(handler);
        let sub = Subscriber {
            id,
            pattern: pattern.clone(),
            handler: handler.clone(),
        };
        self.subscribers
            .write()
            .entry(pattern.clone())
            .or_default()
            .push(sub);

        // 补发缓冲内匹配事件（至少一次语义）。
        let buffered: Vec<(String, Value)> = self
            .replay
            .lock()
            .iter()
            .filter(|(name, _)| pattern_matches(&pattern, name))
            .cloned()
            .collect();
        for (name, payload) in buffered {
            handler(&name, &payload);
        }
        Subscription(id)
    }

    /// 退订。
    pub fn unsubscribe(&self, sub: Subscription) {
        let mut map = self.subscribers.write();
        for subs in map.values_mut() {
            subs.retain(|s| s.id != sub.0);
        }
        map.retain(|_, v| !v.is_empty());
    }

    /// 发布事件：同步分发 + 入重放缓冲 + 桥接前端。
    pub fn publish(&self, name: &str, payload: Value) {
        let matched: Vec<Handler> = self
            .subscribers
            .read()
            .iter()
            .filter(|(pattern, _)| pattern_matches(pattern, name))
            .flat_map(|(_, subs)| subs.iter().map(|s| s.handler.clone()))
            .collect();
        for h in matched {
            h(name, &payload);
        }

        {
            let mut replay = self.replay.lock();
            replay.push_back((name.to_string(), payload.clone()));
            while replay.len() > REPLAY_CAP {
                replay.pop_front();
            }
        }

        if let Some(app) = self.app.lock().as_ref() {
            // Tauri 事件名不允许 `.`（仅 [a-zA-Z0-9-/: _]），桥接前端时统一映射为连字符。
            let frontend_name = name.replace('.', "-");
            if let Err(e) = app.emit(&frontend_name, &payload) {
                eprintln!("[events] failed to bridge `{frontend_name}` to frontend: {e}");
            }
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// 模式匹配：精确名，或以 `.*` 结尾的前缀匹配。
pub fn pattern_matches(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        name == prefix || name.starts_with(&format!("{prefix}."))
    } else {
        pattern == name
    }
}
