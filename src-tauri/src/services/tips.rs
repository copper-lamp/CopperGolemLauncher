//! 提示服务：加载期间向用户展示的随机提示（tips）。
//!
//! 定位：铜内核提供给各模块复用的**通用能力**。内核启动加载页与模块的数据加载态
//! 都经此服务取一条提示，避免用户面对空白界面等待。
//!
//! 数据源与 i18n 强绑定（单一数据源）：
//! - 提示文案存放在内核语言包的 `tips.items.<序号>` 命名空间下
//!   （`frontend/src/locales/{zh-CN,en-US}.json`），与其它内核文案同源同回退链。
//! - 本服务只负责**挑选 key**（`tips.items.3`），文案由前端 `t(key)` 渲染。
//!   这样语言切换时正在显示的提示会立即跟随切换，无需重新请求。
//!
//! 挑选策略：在「当前语言可用 key 集合」内做**不连续重复**的随机。
//! 记忆上一次返回的 key，只要集合长度大于 1 就重新抽取，避免同一条提示连播两轮。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::services::i18n::I18nService;

/// 提示 key 的命名空间前缀。
pub const TIPS_NAMESPACE: &str = "tips.items";

/// 一条提示（返回给前端的视图）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TipView {
    /// i18n 键，如 `tips.items.3`；前端用 `t(key)` 取文案。
    pub key: String,
    /// 当前语言下的文案（服务端按当前 locale 解析，供无 i18n 场景直接使用）。
    pub text: String,
}

/// 提示挑选器：在候选 key 集合中做「不连续重复」的伪随机挑选。
///
/// 独立于 [`TipsService`]，因为它不依赖 i18n —— 这样挑选策略可以被直接单测
/// （无需构造内核服务），避免测试里复制一份实现、反而测不到真实代码。
struct TipPicker {
    /// 上一次返回的 key。
    last_key: parking_lot::Mutex<Option<String>>,
    /// 随机数状态（xorshift64*），避免为一个轻量挑选引入 rand 依赖。
    rng_state: AtomicU64,
}

impl TipPicker {
    fn new(seed: u64) -> Self {
        Self {
            last_key: parking_lot::Mutex::new(None),
            // 种子为 0 时 xorshift 会永远停在 0，故强制置最低位。
            rng_state: AtomicU64::new(seed | 1),
        }
    }

    /// 挑选一个与上次不同的 key。
    ///
    /// 集合长度 > 1 时**保证**返回值不等于上次；长度为 1 时只能重复返回该元素。
    fn pick(&self, keys: &[String]) -> String {
        if keys.len() == 1 {
            return keys[0].clone();
        }
        let last = self.last_key.lock().clone();
        // 随机重试若干轮；极端情况下（连续命中 last）退化为顺序挑选，
        // 保证「不连续重复」这一契约在任何随机序列下都成立。
        for _ in 0..keys.len() {
            let idx = (self.next_u64() % keys.len() as u64) as usize;
            let candidate = &keys[idx];
            if Some(candidate) != last.as_ref() {
                let picked = candidate.clone();
                *self.last_key.lock() = Some(picked.clone());
                return picked;
            }
        }
        let fallback = keys
            .iter()
            .find(|k| Some(*k) != last.as_ref())
            .cloned()
            .unwrap_or_else(|| keys[0].clone());
        *self.last_key.lock() = Some(fallback.clone());
        fallback
    }

    /// xorshift64* 取一个伪随机数（非密码学用途，仅用于挑选提示）。
    fn next_u64(&self) -> u64 {
        let mut x = self.rng_state.load(Ordering::Relaxed);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng_state.store(x, Ordering::Relaxed);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// 提示服务。
pub struct TipsService {
    i18n: Arc<I18nService>,
    picker: TipPicker,
}

impl TipsService {
    pub fn new(i18n: Arc<I18nService>) -> Self {
        // 种子取当前时间 + 进程 id 的混合，保证每次启动序列不同。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        let seed = nanos ^ ((std::process::id() as u64) << 32);
        Self {
            i18n,
            picker: TipPicker::new(seed),
        }
    }

    /// 当前语言下可用的提示 key 列表（按序号自然排序）。
    ///
    /// 语言包缺失 `tips.items` 时返回空列表（调用方须能容忍空集合）。
    pub fn keys(&self) -> Vec<String> {
        let locale = self.i18n.current_locale();
        let catalog = self.i18n.catalog(&locale);
        let mut keys = collect_keys(&catalog);
        sort_by_numeric_suffix(&mut keys);
        keys
    }

    /// 取一条随机提示。集合为空时返回 `None`（前端回退到无提示的加载态）。
    pub fn next(&self) -> Option<TipView> {
        let keys = self.keys();
        if keys.is_empty() {
            return None;
        }
        let picked = self.picker.pick(&keys);
        let locale = self.i18n.current_locale();
        let text = self.i18n.t(&locale, &picked);
        Some(TipView { key: picked, text })
    }
}

/// 从语言包目录中收集提示 key（仅接受值为字符串的条目）。
///
/// 独立成自由函数，使其可在不启动内核服务的前提下单测。
pub fn collect_keys(catalog: &serde_json::Value) -> Vec<String> {
    catalog
        .get("tips")
        .and_then(|tips| tips.get("items"))
        .and_then(|items| items.as_object())
        .map(|items| {
            let mut keys: Vec<String> = items
                .iter()
                .filter(|(_, v)| v.is_string())
                .map(|(k, _)| format!("{TIPS_NAMESPACE}.{k}"))
                .collect();
            keys.sort();
            keys
        })
        .unwrap_or_default()
}

/// 按 key 末尾数字排序，使 `tips.items.10` 排在 `tips.items.9` 之后而非之前。
fn sort_by_numeric_suffix(keys: &mut [String]) {
    keys.sort_by(|a, b| {
        let na = a.rsplit('.').next().and_then(|s| s.parse::<u64>().ok());
        let nb = b.rsplit('.').next().and_then(|s| s.parse::<u64>().ok());
        match (na, nb) {
            (Some(x), Some(y)) => x.cmp(&y),
            _ => a.cmp(b),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_picker() -> TipPicker {
        // 固定种子，测试可复现。
        TipPicker::new(0x2545_F491_4F6C_DD1D)
    }

    fn keys_of(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("tips.items.{i}")).collect()
    }

    #[test]
    fn collect_keys_reads_string_items_only() {
        let catalog = serde_json::json!({
            "tips": {
                "items": {
                    "1": "第一条",
                    "2": "第二条",
                    "bad": { "nested": true }
                }
            }
        });
        let keys = collect_keys(&catalog);
        assert_eq!(keys, vec!["tips.items.1", "tips.items.2"]);
    }

    #[test]
    fn collect_keys_tolerates_missing_namespace() {
        assert!(collect_keys(&serde_json::json!({})).is_empty());
        assert!(collect_keys(&serde_json::json!({ "tips": {} })).is_empty());
    }

    #[test]
    fn keys_are_sorted_by_numeric_suffix() {
        let mut keys = vec![
            "tips.items.10".to_string(),
            "tips.items.2".to_string(),
            "tips.items.9".to_string(),
        ];
        sort_by_numeric_suffix(&mut keys);
        assert_eq!(keys, vec!["tips.items.2", "tips.items.9", "tips.items.10"]);
    }

    #[test]
    fn picks_never_repeat_consecutively() {
        let picker = new_picker();
        let keys = keys_of(3);
        let mut last: Option<String> = None;
        for _ in 0..500 {
            let picked = picker.pick(&keys);
            assert!(keys.contains(&picked), "挑出的 key 必须在候选集合内");
            if let Some(prev) = &last {
                assert_ne!(prev, &picked, "连续两次返回了同一条提示");
            }
            last = Some(picked);
        }
    }

    #[test]
    fn single_tip_is_returned_repeatedly() {
        let picker = new_picker();
        let keys = keys_of(1);
        for _ in 0..5 {
            assert_eq!(picker.pick(&keys), "tips.items.1");
        }
    }

    #[test]
    fn every_key_is_reachable() {
        // 2000 次抽取应覆盖全部候选（3 条），确认随机分布未退化。
        let picker = new_picker();
        let keys = keys_of(3);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..2000 {
            seen.insert(picker.pick(&keys));
        }
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn seed_zero_does_not_stall() {
        // 种子为 0 时 xorshift 会永远输出 0；`new` 强制置最低位以防退化。
        let picker = TipPicker::new(0);
        let keys = keys_of(3);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            seen.insert(picker.pick(&keys));
        }
        assert!(seen.len() > 1, "随机序列退化成了固定值");
    }
}
