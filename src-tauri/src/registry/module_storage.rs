//! 模块私有存储：每个模块一个隔离命名空间，用于持久化自身数据。
//!
//! # 为什么不让模块直接用数据库
//!
//! 内核的 `DatabaseService` 暴露的是 `with_conn(闭包)`，闭包无法跨进程传递；而把 SQL
//! 直接开放给插件，又会绕过内核的表结构与迁移约束（插件可以读别人的表）。
//!
//! 这里改为**有界的具名键值操作**：模块只能读写自己的命名空间，写入总量有上限，
//! 内核因此始终掌握数据形状（JSON）、落点（自身目录）与容量。模板的持久化需求
//! （CRUD 若干条记录）用这组操作即可表达，无需把连接代理过去。
//!
//! # 数据安全
//!
//! - 落点固定在 `<data_dir>/module_data/<module_id>/storage.json`，模块 id 在协议层
//!   已按两段式规则校验，不构成路径注入。
//! - 写入先落临时文件再原子替换，进程中断不会留下半截 JSON。
//! - 文件损坏时**报错而不是静默重置**：宁可让用户看到错误，也不能悄悄吞掉数据。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde_json::Value;

/// 单个模块可持久化的数据总量上限。
const MAX_NAMESPACE_BYTES: usize = 4 * 1024 * 1024;
/// 单个键的长度上限。
pub const MAX_KEY_BYTES: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage key must be non-empty and at most 256 bytes")]
    InvalidKey,
    #[error("module `{module_id}` storage exceeds the 4 MiB limit")]
    QuotaExceeded { module_id: String },
    #[error("module `{module_id}` storage file is corrupt: {detail}")]
    Corrupt { module_id: String, detail: String },
    #[error("module storage I/O failed: {0}")]
    Io(String),
}

impl StorageError {
    /// 透出给插件的能力错误码。分码而不是一律 `storage_error`，插件才能区分
    /// 「我的参数不对」与「我的数据坏了」。
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidKey => "storage_invalid_key",
            Self::QuotaExceeded { .. } => "storage_quota_exceeded",
            Self::Corrupt { .. } => "storage_corrupt",
            Self::Io(_) => "storage_io_error",
        }
    }
}

/// 模块私有存储。按模块懒加载并缓存，写入后立即持久化。
pub struct ModuleStorage {
    root: PathBuf,
    cache: Mutex<BTreeMap<String, BTreeMap<String, Value>>>,
}

impl ModuleStorage {
    /// `data_dir` 为启动器数据目录；模块数据落在其下的 `module_data/` 中。
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("module_data"),
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn get(&self, module_id: &str, key: &str) -> Result<Option<Value>, StorageError> {
        validate_key(key)?;
        let mut cache = self.cache.lock();
        let namespace = self.namespace_mut(&mut cache, module_id)?;
        Ok(namespace.get(key).cloned())
    }

    /// 写入一个键。超限时缓存与磁盘都不改动，避免"写一半再回滚"。
    pub fn set(&self, module_id: &str, key: &str, value: Value) -> Result<(), StorageError> {
        validate_key(key)?;
        let mut cache = self.cache.lock();
        let namespace = self.namespace_mut(&mut cache, module_id)?;

        let mut candidate = namespace.clone();
        candidate.insert(key.to_owned(), value);
        let encoded = encode_namespace(module_id, &candidate)?;
        if encoded.len() > MAX_NAMESPACE_BYTES {
            return Err(StorageError::QuotaExceeded {
                module_id: module_id.to_owned(),
            });
        }

        // 先落盘再更新缓存：只有持久化成功，内存视图才前进。
        self.persist(module_id, &encoded)?;
        *namespace = candidate;
        Ok(())
    }

    /// 删除一个键，返回它此前是否存在。
    pub fn remove(&self, module_id: &str, key: &str) -> Result<bool, StorageError> {
        validate_key(key)?;
        let mut cache = self.cache.lock();
        let namespace = self.namespace_mut(&mut cache, module_id)?;
        if !namespace.contains_key(key) {
            return Ok(false);
        }

        let mut candidate = namespace.clone();
        candidate.remove(key);
        let encoded = encode_namespace(module_id, &candidate)?;
        self.persist(module_id, &encoded)?;
        *namespace = candidate;
        Ok(true)
    }

    /// 列出键（按字典序）。`prefix` 为空时列出全部。
    pub fn list(&self, module_id: &str, prefix: &str) -> Result<Vec<String>, StorageError> {
        let mut cache = self.cache.lock();
        let namespace = self.namespace_mut(&mut cache, module_id)?;
        Ok(namespace
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }

    fn namespace_mut<'a>(
        &self,
        cache: &'a mut BTreeMap<String, BTreeMap<String, Value>>,
        module_id: &str,
    ) -> Result<&'a mut BTreeMap<String, Value>, StorageError> {
        if !cache.contains_key(module_id) {
            let loaded = self.load(module_id)?;
            cache.insert(module_id.to_owned(), loaded);
        }
        Ok(cache
            .get_mut(module_id)
            .expect("namespace was inserted above"))
    }

    fn load(&self, module_id: &str) -> Result<BTreeMap<String, Value>, StorageError> {
        let path = self.storage_file(module_id);
        match std::fs::read(&path) {
            Ok(raw) => serde_json::from_slice(&raw).map_err(|error| StorageError::Corrupt {
                module_id: module_id.to_owned(),
                detail: error.to_string(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(StorageError::Io(error.to_string())),
        }
    }

    fn persist(&self, module_id: &str, encoded: &[u8]) -> Result<(), StorageError> {
        let directory = self.root.join(module_id);
        std::fs::create_dir_all(&directory).map_err(|error| StorageError::Io(error.to_string()))?;

        let target = directory.join("storage.json");
        let temporary = directory.join("storage.json.tmp");
        std::fs::write(&temporary, encoded).map_err(|error| StorageError::Io(error.to_string()))?;
        // 原子替换：中断只会留下 .tmp，不会产生半截 storage.json。
        std::fs::rename(&temporary, &target).map_err(|error| StorageError::Io(error.to_string()))
    }

    fn storage_file(&self, module_id: &str) -> PathBuf {
        self.root.join(module_id).join("storage.json")
    }
}

fn validate_key(key: &str) -> Result<(), StorageError> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err(StorageError::InvalidKey);
    }
    Ok(())
}

fn encode_namespace(
    module_id: &str,
    namespace: &BTreeMap<String, Value>,
) -> Result<Vec<u8>, StorageError> {
    serde_json::to_vec(namespace).map_err(|error| StorageError::Corrupt {
        module_id: module_id.to_owned(),
        detail: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn storage(tag: &str) -> (ModuleStorage, PathBuf) {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "cgl-storage-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        (ModuleStorage::new(&root), root)
    }

    #[test]
    fn stores_reads_and_removes_values() {
        let (store, root) = storage("crud");
        let id = "copper-lamp.demo-tools";

        assert_eq!(store.get(id, "note").unwrap(), None);
        store.set(id, "note", json!({ "title": "hello" })).unwrap();
        assert_eq!(
            store.get(id, "note").unwrap(),
            Some(json!({ "title": "hello" }))
        );
        assert!(store.remove(id, "note").unwrap());
        assert!(!store.remove(id, "note").unwrap());
        assert_eq!(store.get(id, "note").unwrap(), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn data_survives_a_restart() {
        let (store, root) = storage("persist");
        let id = "copper-lamp.demo-tools";
        store.set(id, "k", json!(7)).unwrap();
        drop(store);

        let reopened = ModuleStorage::new(&root);
        assert_eq!(reopened.get(id, "k").unwrap(), Some(json!(7)));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn namespaces_are_isolated_between_modules() {
        let (store, root) = storage("isolation");
        store.set("copper-lamp.alpha", "k", json!("alpha")).unwrap();
        store.set("copper-lamp.beta", "k", json!("beta")).unwrap();

        assert_eq!(
            store.get("copper-lamp.alpha", "k").unwrap(),
            Some(json!("alpha"))
        );
        assert_eq!(
            store.get("copper-lamp.beta", "k").unwrap(),
            Some(json!("beta"))
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_filters_by_prefix_and_sorts() {
        let (store, root) = storage("list");
        let id = "copper-lamp.demo-tools";
        for key in ["note:2", "note:1", "other"] {
            store.set(id, key, json!(null)).unwrap();
        }

        assert_eq!(
            store.list(id, "note:").unwrap(),
            vec!["note:1".to_owned(), "note:2".to_owned()]
        );
        assert_eq!(store.list(id, "").unwrap().len(), 3);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_an_empty_or_oversized_key() {
        let (store, root) = storage("keys");
        let id = "copper-lamp.demo-tools";

        assert!(matches!(
            store.get(id, ""),
            Err(StorageError::InvalidKey)
        ));
        let oversized = "k".repeat(MAX_KEY_BYTES + 1);
        assert!(matches!(
            store.set(id, &oversized, json!(1)),
            Err(StorageError::InvalidKey)
        ));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_writes_beyond_the_quota_without_touching_existing_data() {
        let (store, root) = storage("quota");
        let id = "copper-lamp.demo-tools";
        store.set(id, "kept", json!("keep me")).unwrap();

        let huge = "x".repeat(MAX_NAMESPACE_BYTES);
        assert!(matches!(
            store.set(id, "huge", json!(huge)),
            Err(StorageError::QuotaExceeded { .. })
        ));
        // 超限写入不得破坏既有数据。
        assert_eq!(store.get(id, "kept").unwrap(), Some(json!("keep me")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_files_are_reported_rather_than_silently_reset() {
        let (store, root) = storage("corrupt");
        let id = "copper-lamp.demo-tools";
        let file = root.join("module_data").join(id).join("storage.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"{ not json").unwrap();

        assert!(matches!(store.get(id, "k"), Err(StorageError::Corrupt { .. })));

        let _ = std::fs::remove_dir_all(&root);
    }
}
