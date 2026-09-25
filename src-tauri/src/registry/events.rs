//! 事件总线：模块间广播式联动中介。
//!
//! 语义：
//! - 发布 / 订阅；订阅可带通配后缀（`download.*` 匹配 `download.created` 等）。
//! - **至少一次**：未就绪订阅者错过的事件缓存在有界重放缓冲，订阅时补发
//!   （[`EventBus::subscribe`]）。附加模块走 [`EventBus::subscribe_from_now`]，
//!   只收订阅之后的事件，**不补发历史**——理由见该方法的注释。
//! - 内核发布的同名事件自动桥接到前端（Tauri `emit`），前端可 `listen` 同名事件。
//!
//! 附加模块的推送链路（订阅 → 白名单过滤 → 有界队列 → 每模块推送线程）在
//! [`crate::registry::addon_events`]，本文件只负责"订阅与分发"。**这是刻意的分工**：
//! `publish` 是**同步**分发（handler 跑在发布方线程上），因此 handler 里绝不能有
//! 阻塞式 IO，否则会把发布方线程一起拖住。

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
        self.register(pattern, handler, true)
    }

    /// 订阅事件，但**不补发**重放缓冲里的历史事件：只收订阅之后发布的事件。
    ///
    /// 附加模块走这条路径。对进程内模块，「晚订阅不漏事件」是有用的保证；但对子进程
    /// 模块，补发历史等于「刚启动就收到启动前的历史」——模块无从区分"刚发生"与
    /// "补发的旧事"，会让 `version.installed` 这类带副作用的处理被重复执行。
    /// 故附加模块的订阅范围从订阅那一刻起算。这是刻意的语义分叉。
    pub fn subscribe_from_now(
        &self,
        pattern: impl Into<String>,
        handler: impl Fn(&str, &Value) + Send + Sync + 'static,
    ) -> Subscription {
        self.register(pattern, handler, false)
    }

    /// 订阅的公共实现：`replay_backlog` 决定是否补发重放缓冲里的匹配事件。
    fn register(
        &self,
        pattern: impl Into<String>,
        handler: impl Fn(&str, &Value) + Send + Sync + 'static,
        replay_backlog: bool,
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

        if replay_backlog {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    /// 计数用的订阅者：只关心"收到几条、内容是什么"。
    struct Recorder {
        hits: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                hits: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn hits(&self) -> Vec<String> {
            self.hits.lock().unwrap().clone()
        }

        fn counter(&self) -> impl Fn(&str, &Value) + Send + Sync + 'static {
            let hits = Arc::clone(&self.hits);
            move |name: &str, _payload: &Value| {
                hits.lock().unwrap().push(name.to_string());
            }
        }
    }

    #[test]
    fn pattern_matching_rules_are_explicit() {
        assert!(pattern_matches("*", "anything.at.all"));
        assert!(pattern_matches("download.*", "download.created"));
        assert!(pattern_matches("download.*", "download"));
        assert!(!pattern_matches("download.*", "downloads.created"));
        assert!(pattern_matches("download.created", "download.created"));
        assert!(!pattern_matches("download.created", "download.removed"));
    }

    #[test]
    fn subscribe_replays_buffered_events() {
        let bus = EventBus::new();
        bus.publish("download.created", json!({ "slug": "1.21.0" }));

        let recorder = Recorder::new();
        bus.subscribe("download.*", recorder.counter());

        assert_eq!(recorder.hits(), vec!["download.created".to_string()]);
    }

    #[test]
    fn subscribe_from_now_does_not_replay_buffered_events() {
        let bus = EventBus::new();
        bus.publish("version.installed", json!({ "slug": "1.21.0" }));

        let recorder = Recorder::new();
        bus.subscribe_from_now("version.*", recorder.counter());

        // 附加模块不能收到"订阅之前"的历史：模块无从区分它与刚发生的事件，
        // 重复执行带副作用的处理正是要避免的。
        assert!(recorder.hits().is_empty(), "不得补发历史事件");

        // 订阅之后发布的事件照收。
        bus.publish("version.installed", json!({ "slug": "1.21.1" }));
        assert_eq!(recorder.hits(), vec!["version.installed".to_string()]);
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let bus = EventBus::new();
        let recorder = Recorder::new();
        let subscription = bus.subscribe_from_now("demo.*", recorder.counter());

        bus.publish("demo.activity", json!({}));
        bus.unsubscribe(subscription);
        bus.publish("demo.activity", json!({}));

        assert_eq!(recorder.hits().len(), 1, "退订后不得再收到事件");
    }

    #[test]
    fn replay_buffer_is_bounded() {
        let bus = EventBus::new();
        for index in 0..(REPLAY_CAP + 8) {
            bus.publish("demo.tick", json!({ "n": index }));
        }

        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        bus.subscribe("demo.*", move |_name, _payload| {
            counter.fetch_add(1, Ordering::Relaxed);
        });

        assert_eq!(seen.load(Ordering::Relaxed), REPLAY_CAP);
    }
}
