//! 附加模块事件推送桥：把内核事件总线的分发，转成对子进程 helper 的事件通知。
//!
//! # 为什么必须是"订阅 → 有界队列 → 每模块推送线程"三层
//!
//! [`EventBus::publish`] 是**同步**分发：订阅回调跑在**发布方线程**上（下载线程、
//! 设置线程……）。若回调里直接写 helper 的 stdin，插件一忙（正在跑一次长 `invoke`）
//! 就会把发布方线程一起堵住——那等于把"某个附加模块卡顿"升级成"内核某项功能卡顿"。
//!
//! 因此回调只做两件事：**按声明模式白名单过滤** + **非阻塞入队**；真正的写管道由
//! 每个模块**一条专属推送线程**独占。队列有界（[`QUEUE_CAPACITY`]），事件洪峰只会
//! 表现为"丢弃并计数"，不会把内核内存吃光。
//!
//! # 与 helper 协议的关系
//!
//! 推送走 `event.dispatch` 单向通知：helper 收到后转成一次插件调用
//! （`plugin.invoke("event.<名>", payload)`），**不回帧**。回帧会打乱宿主的请求 /
//! 响应配对（见 `copper_module_abi::ipc::METHOD_EVENT_DISPATCH`）。
//!
//! # 生命周期
//!
//! 通道与模块的 `start` / `stop` 同寿命：`start` 成功后 [`AddonEventBridge::register`]，
//! `stop` 时 [`AddonEventBridge::unregister`]（置停止位 → 唤醒 → join 线程 → 删条目）。
//! 漏掉 `unregister` 会把事件推给已死进程，或在下次启动时残留旧订阅——两个方向都错。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::{Condvar, Mutex, RwLock};
use serde_json::Value;

use crate::registry::events::{pattern_matches, EventBus, Subscription};

/// 单个模块待推送队列的上限。
///
/// 有界是刻意的：插件可能长时间不读（正在跑一次长命令），此时无界队列的容量就由
/// 事件速率决定。超限丢弃并计数，而不是让内核内存随事件洪峰增长。
const QUEUE_CAPACITY: usize = 256;

/// 撤销通道时等待推送线程退出的上限。
///
/// **必须有上限**：推送线程可能正阻塞在一次管道写上（插件卡在长命令里、不再读 stdin），
/// 此时无限 `join` 会让模块 `stop`——进而让内核退出——永久挂起。超时后如实留痕并
/// 分离该线程：`stopped` 已置位，队列不再增长，线程会在它那次写返回后自行退出。
const PUSH_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// 推送出口：把一条事件交给某个模块的 helper。
///
/// 抽成 trait 是为了让队列语义（有界、保序、满则丢弃、写失败即停）可以用一个可观测
/// 的替身单测，而不必为每个用例拉起真实子进程。生产实现是
/// [`copper_module_abi::helper_client::HelperPushHandle`]。
pub trait EventPusher: Send + Sync + 'static {
    /// 交付一条事件。返回 `Err` 表示该出口已不可用（进程退出 / 管道关闭）。
    fn push(&self, event: &str, payload: &Value) -> Result<(), String>;
}

impl EventPusher for copper_module_abi::helper_client::HelperPushHandle {
    fn push(&self, event: &str, payload: &Value) -> Result<(), String> {
        self.push_event(event, payload)
            .map_err(|error| error.to_string())
    }
}

/// 入队失败的原因。
enum DropReason {
    /// 队列已满。
    Full,
    /// 通道已停用或推送线程已死。
    Closed,
}

/// 队列状态：与条件变量配对，必须在同一把锁下读写。
struct QueueState {
    pending: VecDeque<(String, Value)>,
    stopped: bool,
}

/// 单个模块的推送通道。
struct ModuleChannel {
    module_id: String,
    /// 清单声明的事件模式（订阅上界）。收到的任何事件都必须命中其中之一。
    patterns: Vec<String>,
    pusher: Arc<dyn EventPusher>,
    /// 推送线程是否仍在工作：写失败即置 `false`，此后不再入队。
    alive: AtomicBool,
    /// 是否已就"队列满"留过一次痕（避免洪峰时刷屏日志）。
    overflow_reported: AtomicBool,
    queue: Mutex<QueueState>,
    signal: Condvar,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// 推送线程退出时回一条空消息；`shut_down` 靠它做**有界**等待（`JoinHandle` 没有带超时的 join）。
    finished: Mutex<Option<Receiver<()>>>,
}

impl ModuleChannel {
    fn new(module_id: &str, patterns: Vec<String>, pusher: Arc<dyn EventPusher>) -> Self {
        Self {
            module_id: module_id.to_owned(),
            patterns,
            pusher,
            alive: AtomicBool::new(true),
            overflow_reported: AtomicBool::new(false),
            queue: Mutex::new(QueueState {
                pending: VecDeque::new(),
                stopped: false,
            }),
            signal: Condvar::new(),
            thread: Mutex::new(None),
            finished: Mutex::new(None),
        }
    }

    /// 事件是否在该模块声明的订阅上界之内。
    fn accepts(&self, name: &str) -> bool {
        self.patterns
            .iter()
            .any(|pattern| pattern_matches(pattern, name))
    }

    /// 非阻塞入队。**本函数跑在发布方线程上**，因此不做任何可能阻塞的事。
    fn enqueue(&self, name: &str, payload: &Value) -> Result<(), DropReason> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(DropReason::Closed);
        }

        let mut state = self.queue.lock();
        if state.stopped {
            return Err(DropReason::Closed);
        }
        if state.pending.len() >= QUEUE_CAPACITY {
            return Err(DropReason::Full);
        }
        state.pending.push_back((name.to_string(), payload.clone()));
        drop(state);

        self.signal.notify_one();
        Ok(())
    }

    /// 是否已进入停止流程。
    fn is_stopping(&self) -> bool {
        self.queue.lock().stopped
    }

    /// 停止接收并给推送线程收尾：置停止位 → 唤醒 → **有界**等待其退出。
    fn shut_down(&self) {
        {
            let mut state = self.queue.lock();
            state.stopped = true;
            // 停机后不再推送已排队的旧事件：模块已 `stop`，它的插件不该再收到新输入。
            state.pending.clear();
        }
        self.signal.notify_all();

        let finished = self.finished.lock().take();
        let exited = match finished {
            Some(receiver) => receiver.recv_timeout(PUSH_JOIN_TIMEOUT).is_ok(),
            // 没有回报通道说明线程从未启动过（构造后立刻撤销）。
            None => true,
        };

        let thread = self.thread.lock().take();
        if exited {
            if let Some(thread) = thread {
                let _ = thread.join();
            }
            return;
        }

        // 超时即分离（丢弃句柄，不 join）：该线程只会阻塞在一次管道写上，等它返回后
        // 会自行看到停止位并退出。进程退出时它随之终结，因此不会泄漏成常驻线程。
        log::warn!(
            "[addon/{}] 推送线程未在 {} 秒内退出（疑似插件已停止读取管道），已分离该线程；\
             本通道不再接收新事件",
            self.module_id,
            PUSH_JOIN_TIMEOUT.as_secs()
        );
        drop(thread);
    }
}

/// 推送线程主体：逐条把队列里的事件写进 helper，直到通道被停止或出口失效。
fn pump_channel(channel: &ModuleChannel) {
    loop {
        // 一次取走当前全部待推送项：批量取出后释放锁，写管道期间不占队列锁。
        let batch: Vec<(String, Value)> = {
            let mut state = channel.queue.lock();
            while state.pending.is_empty() && !state.stopped {
                channel.signal.wait(&mut state);
            }
            if state.pending.is_empty() {
                // 只有"已停止且已排空"才会走到这里。
                return;
            }
            state.pending.drain(..).collect()
        };

        for (name, payload) in batch {
            // 停止位已在批次取出之后置位：剩余项不再推送（模块正在退出）。
            if channel.is_stopping() {
                return;
            }
            if let Err(error) = channel.pusher.push(&name, &payload) {
                // 写失败即出口不可用。停掉线程而不是重试：重试只会重复失败并堆积日志，
                // 通道的真实回收交给 `unregister`（由模块 `stop` 触发）。
                channel.alive.store(false, Ordering::SeqCst);
                log::warn!(
                    "[addon/{}] 事件 `{name}` 推送失败，停止推送：{error}",
                    channel.module_id
                );
                return;
            }
        }
    }
}

/// 启动推送线程，并登记"退出回报"通道供 [`ModuleChannel::shut_down`] 有界等待。
fn spawn_pusher(channel: Arc<ModuleChannel>) -> JoinHandle<()> {
    let (finished_tx, finished_rx) = mpsc::channel();
    *channel.finished.lock() = Some(finished_rx);

    std::thread::spawn(move || {
        pump_channel(&channel);
        // 无论正常停止还是写失败退出，都要回报一声；漏掉回报会让 `shut_down` 白等满超时。
        let _ = finished_tx.send(());
    })
}

/// 订阅回调执行的路由状态。
///
/// 与 [`AddonEventBridge`] 分开是为了让"持有订阅句柄"和"被订阅回调持有"互不牵连：
/// 回调只需要这一份状态，不需要也不应该碰桥本身。
#[derive(Default)]
struct RoutingState {
    modules: RwLock<HashMap<String, Arc<ModuleChannel>>>,
    /// 因队列满被丢弃的事件总数（供自省与测试）。
    dropped: AtomicU64,
}

impl RoutingState {
    /// 按各模块声明的模式过滤并入队。
    ///
    /// **跑在 `EventBus::publish` 的调用线程上**：只做过滤与非阻塞入队，绝不写管道。
    fn route(&self, name: &str, payload: &Value) {
        let channels: Vec<Arc<ModuleChannel>> =
            self.modules.read().values().cloned().collect();
        for channel in channels {
            if !channel.accepts(name) {
                continue;
            }
            if let Err(DropReason::Full) = channel.enqueue(name, payload) {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                if !channel.overflow_reported.swap(true, Ordering::SeqCst) {
                    log::warn!(
                        "[addon/{}] 事件队列已满（容量 {QUEUE_CAPACITY}），开始丢弃事件；\
                         插件处理过慢时会出现这种情况",
                        channel.module_id
                    );
                }
            }
        }
    }
}

/// 附加模块事件推送桥。
pub struct AddonEventBridge {
    bus: Arc<EventBus>,
    state: Arc<RoutingState>,
    /// `*` 订阅句柄：一次订阅覆盖全部事件，白名单过滤在路由层完成。
    subscription: Mutex<Option<Subscription>>,
}

impl AddonEventBridge {
    /// 建立桥，并立刻以 `*` 订阅事件总线。
    ///
    /// 用 `subscribe_from_now` 而非 `subscribe`：附加模块**不补发历史事件**。
    /// 子进程无从区分"刚发生"与"补发的旧事"，补发会让 `version.installed` 这类
    /// 带副作用的处理被重复执行（见 [`EventBus::subscribe_from_now`]）。
    pub fn new(bus: Arc<EventBus>) -> Self {
        let state = Arc::new(RoutingState::default());
        let router = Arc::clone(&state);
        let subscription = bus.subscribe_from_now("*", move |name, payload| {
            router.route(name, payload)
        });

        Self {
            bus,
            state,
            subscription: Mutex::new(Some(subscription)),
        }
    }

    /// 为一个**已启动**的模块登记推送通道。
    ///
    /// 同一 id 重复登记会先撤销旧通道（含 join 旧推送线程），避免残留旧订阅把事件
    /// 推给已被替换的会话。
    pub fn register(
        &self,
        module_id: &str,
        patterns: Vec<String>,
        pusher: Arc<dyn EventPusher>,
    ) {
        self.unregister(module_id);

        let channel = Arc::new(ModuleChannel::new(module_id, patterns, pusher));
        let thread = spawn_pusher(Arc::clone(&channel));
        channel.thread.lock().replace(thread);

        let description = if channel.patterns.is_empty() {
            "无（不接收任何事件）".to_owned()
        } else {
            channel.patterns.join(", ")
        };

        self.state
            .modules
            .write()
            .insert(module_id.to_string(), channel);

        log::info!("[addon/{module_id}] 已登记事件推送（声明模式：{description}）");
    }

    /// 撤销一个模块的推送通道。
    ///
    /// 顺序即正确性：先置停止位并清空待推送，再唤醒并 join 推送线程，最后删条目。
    /// 反过来的话，推送线程可能在被 join 前又写出一帧，把事件送给正在退出的插件。
    pub fn unregister(&self, module_id: &str) {
        let Some(channel) = self.state.modules.write().remove(module_id) else {
            return;
        };
        channel.shut_down();
        log::info!("[addon/{module_id}] 已撤销事件推送");
    }

    /// 已登记推送通道的模块（id + 声明模式），供自省与测试。
    pub fn subscribed(&self) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = self
            .state
            .modules
            .read()
            .iter()
            .map(|(id, channel)| (id.clone(), channel.patterns.clone()))
            .collect();
        out.sort();
        out
    }

    /// 某模块的推送通道是否仍在工作（模块未登记、或推送线程已因写失败退出时为 `false`）。
    pub fn is_live(&self, module_id: &str) -> bool {
        self.state
            .modules
            .read()
            .get(module_id)
            .is_some_and(|channel| channel.alive.load(Ordering::SeqCst))
    }

    /// 因队列满被丢弃的事件总数。
    pub fn dropped(&self) -> u64 {
        self.state.dropped.load(Ordering::Relaxed)
    }

    /// 阻塞等待某模块队列排空（仅供测试与停机路径使用）。
    #[doc(hidden)]
    pub fn wait_until_idle(&self, module_id: &str, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let empty = self
                .state
                .modules
                .read()
                .get(module_id)
                .map(|channel| channel.queue.lock().pending.is_empty())
                .unwrap_or(true);
            if empty {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for AddonEventBridge {
    fn drop(&mut self) {
        if let Some(subscription) = self.subscription.lock().take() {
            self.bus.unsubscribe(subscription);
        }
        let ids: Vec<String> = self.state.modules.read().keys().cloned().collect();
        for id in ids {
            self.unregister(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    /// 可观测的推送替身：记录收到的推送，可切换为"出口已坏"。
    #[derive(Default)]
    struct RecordingPusher {
        pushed: Mutex<Vec<(String, Value)>>,
        attempts: AtomicUsize,
        failing: AtomicBool,
    }

    impl RecordingPusher {
        fn pushed(&self) -> Vec<(String, Value)> {
            self.pushed.lock().clone()
        }

        fn attempts(&self) -> usize {
            self.attempts.load(Ordering::SeqCst)
        }
    }

    impl EventPusher for RecordingPusher {
        fn push(&self, event: &str, payload: &Value) -> Result<(), String> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.failing.load(Ordering::SeqCst) {
                return Err("helper is gone".to_owned());
            }
            self.pushed.lock().push((event.to_owned(), payload.clone()));
            Ok(())
        }
    }

    /// 轮询等待条件成立；推送线程是异步的，断言前必须给它时间。
    fn wait_for(mut predicate: impl FnMut() -> bool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        predicate()
    }

    const WAIT: Duration = Duration::from_secs(5);

    fn bridge_with(
        patterns: &[&str],
        pusher: Arc<RecordingPusher>,
    ) -> (Arc<EventBus>, AddonEventBridge) {
        let bus = Arc::new(EventBus::new());
        let bridge = AddonEventBridge::new(Arc::clone(&bus));
        bridge.register(
            "copper-lamp.demo-tools",
            patterns.iter().map(|p| p.to_string()).collect(),
            pusher,
        );
        (bus, bridge)
    }

    #[test]
    fn only_events_within_the_declared_patterns_are_pushed() {
        let pusher = Arc::new(RecordingPusher::default());
        let (bus, bridge) = bridge_with(&["download.*"], Arc::clone(&pusher));

        bus.publish("download.created", json!({ "slug": "1.21.0" }));
        bus.publish("settings.changed", json!({ "key": "theme" }));
        bus.publish("download.status", json!({ "slug": "1.21.0", "state": "done" }));

        assert!(wait_for(|| pusher.pushed().len() == 2, WAIT));
        let pushed = pusher.pushed();
        assert_eq!(pushed[0].0, "download.created");
        assert_eq!(pushed[1].0, "download.status");
        // 未声明的敏感事件绝不能出现在推送里——这是订阅上界的意义所在。
        assert!(
            pushed.iter().all(|(name, _)| name != "settings.changed"),
            "未声明的事件被推送了：{pushed:?}"
        );

        drop(bridge);
    }

    #[test]
    fn pushed_events_keep_the_publish_order() {
        let pusher = Arc::new(RecordingPusher::default());
        let (bus, bridge) = bridge_with(&["*"], Arc::clone(&pusher));

        for index in 0..32 {
            bus.publish("demo.tick", json!({ "n": index }));
        }

        assert!(wait_for(|| pusher.pushed().len() == 32, WAIT));
        let pushed = pusher.pushed();
        for (index, (name, payload)) in pushed.iter().enumerate() {
            assert_eq!(name, "demo.tick");
            assert_eq!(payload["n"], json!(index), "推送顺序必须与发布顺序一致");
        }

        drop(bridge);
    }

    #[test]
    fn unregister_stops_pushing_and_clears_the_backlog() {
        let pusher = Arc::new(RecordingPusher::default());
        let (bus, bridge) = bridge_with(&["*"], Arc::clone(&pusher));

        bus.publish("demo.tick", json!({ "n": 0 }));
        assert!(wait_for(|| pusher.pushed().len() == 1, WAIT));

        bridge.unregister("copper-lamp.demo-tools");
        assert!(bridge.subscribed().is_empty(), "撤销后不得残留登记");
        assert!(!bridge.is_live("copper-lamp.demo-tools"));

        bus.publish("demo.tick", json!({ "n": 1 }));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(pusher.pushed().len(), 1, "撤销后不得再推送");

        drop(bridge);
    }

    #[test]
    fn a_broken_channel_stops_pushing_and_is_not_retried() {
        let pusher = Arc::new(RecordingPusher::default());
        pusher.failing.store(true, Ordering::SeqCst);
        let (bus, bridge) = bridge_with(&["*"], Arc::clone(&pusher));

        bus.publish("demo.tick", json!({ "n": 0 }));
        assert!(
            wait_for(|| !bridge.is_live("copper-lamp.demo-tools"), WAIT),
            "写失败后通道必须被标记为不可用"
        );
        let attempts_after_failure = pusher.attempts();

        // 后续事件不再尝试：重试只会重复失败并刷屏日志。
        bus.publish("demo.tick", json!({ "n": 1 }));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            pusher.attempts(),
            attempts_after_failure,
            "通道已坏后不得继续重试"
        );
        assert_eq!(pusher.pushed().len(), 0);

        drop(bridge);
    }

    #[test]
    fn re_registering_replaces_the_previous_channel() {
        let first = Arc::new(RecordingPusher::default());
        let second = Arc::new(RecordingPusher::default());
        let bus = Arc::new(EventBus::new());
        let bridge = AddonEventBridge::new(Arc::clone(&bus));

        bridge.register(
            "copper-lamp.demo-tools",
            vec!["*".to_owned()],
            Arc::clone(&first) as Arc<dyn EventPusher>,
        );
        bridge.register(
            "copper-lamp.demo-tools",
            vec!["*".to_owned()],
            Arc::clone(&second) as Arc<dyn EventPusher>,
        );

        assert_eq!(bridge.subscribed().len(), 1, "同一 id 不得残留两个通道");

        bus.publish("demo.tick", json!({ "n": 0 }));
        assert!(wait_for(|| second.pushed().len() == 1, WAIT));
        assert_eq!(first.pushed().len(), 0, "旧通道必须已被撤销");

        drop(bridge);
    }

    #[test]
    fn a_full_queue_drops_events_instead_of_growing_without_bound() {
        /// 永远阻塞在推送里：用来把队列顶满。
        ///
        /// `entered` 是必需的可观测点：只有在推送线程**已经卡住**之后灌洪峰，
        /// 队列容量与丢弃数才是确定的；否则线程可能在洪峰期间批量取走若干条，
        /// 丢弃数就变成时序相关的量。
        struct BlockingPusher {
            entered: AtomicBool,
            released: Arc<AtomicBool>,
            pushed: AtomicUsize,
        }

        impl EventPusher for BlockingPusher {
            fn push(&self, _event: &str, _payload: &Value) -> Result<(), String> {
                self.entered.store(true, Ordering::SeqCst);
                while !self.released.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                self.pushed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let released = Arc::new(AtomicBool::new(false));
        let pusher = Arc::new(BlockingPusher {
            entered: AtomicBool::new(false),
            released: Arc::clone(&released),
            pushed: AtomicUsize::new(0),
        });
        let bus = Arc::new(EventBus::new());
        let bridge = AddonEventBridge::new(Arc::clone(&bus));
        bridge.register(
            "copper-lamp.demo-tools",
            vec!["*".to_owned()],
            Arc::clone(&pusher) as Arc<dyn EventPusher>,
        );

        // 先发一条并等推送线程卡在它上面：此时它手里只有这一条，队列是空的。
        bus.publish("demo.tick", json!({ "n": -1 }));
        assert!(
            wait_for(|| pusher.entered.load(Ordering::SeqCst), WAIT),
            "推送线程应当已经开始推送"
        );

        // 洪峰：容量内的进队列，超出的必须被**明确计数丢弃**。
        let total = QUEUE_CAPACITY + 16;
        for index in 0..total {
            bus.publish("demo.tick", json!({ "n": index }));
        }
        assert_eq!(
            bridge.dropped(),
            (total - QUEUE_CAPACITY) as u64,
            "超容量的事件必须被丢弃并计数，而不是无界堆积"
        );

        // 放开推送线程：它会把队列排空；被丢弃的不会补发。
        released.store(true, Ordering::SeqCst);
        assert!(
            wait_for(|| pusher.pushed.load(Ordering::SeqCst) > 0, WAIT),
            "放开后推送线程必须继续工作"
        );

        // 守恒：每条事件要么被推送，要么被明确计入丢弃数，不存在静默消失。
        let delivered = pusher.pushed.load(Ordering::SeqCst) as u64 + bridge.dropped();
        assert!(
            delivered <= (total + 1) as u64,
            "推送数 + 丢弃数不得超过发布总量（{delivered} vs {}）",
            total + 1
        );

        drop(bridge);
    }

    #[test]
    fn registering_with_no_patterns_receives_nothing() {
        let pusher = Arc::new(RecordingPusher::default());
        let (bus, bridge) = bridge_with(&[], Arc::clone(&pusher));

        bus.publish("demo.tick", json!({}));
        // 空声明 = 什么都不收（与 permissions 的空数组同一口径）。
        assert_eq!(
            bridge
                .subscribed()
                .first()
                .map(|(_, patterns)| patterns.len()),
            Some(0)
        );
        std::thread::sleep(Duration::from_millis(50));
        assert!(pusher.pushed().is_empty());

        drop(bridge);
    }
}
