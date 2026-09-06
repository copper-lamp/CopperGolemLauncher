//! 数据库服务：SQLite 全局持久化。
//!
//! 约定：
//! - 核心表命名空间 `core_*`（设置、账户、下载记录等）。
//! - 模块表命名空间 `module_<名>_*`，由各模块通过 `migrate_scope` 提供迁移。
//! - schema 变更走版本迁移（`schema_migrations` 记录），事务内执行，失败回滚。

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};

use crate::error::KernelError;

/// 一次 schema 迁移。
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// 数据库服务。内部持有一把互斥锁保护的连接（同步访问，短临界区）。
pub struct DatabaseService {
    conn: Mutex<Connection>,
}

impl DatabaseService {
    /// 打开（必要时创建）数据库文件并初始化迁移记录表。
    pub fn open(path: &Path) -> Result<Self, KernelError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let service = Self {
            conn: Mutex::new(conn),
        };
        service.exec_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                scope     TEXT    NOT NULL,
                version   INTEGER NOT NULL,
                name      TEXT    NOT NULL,
                applied_at TEXT   NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (scope, version)
            )",
        )?;
        Ok(service)
    }

    /// 在作用域内执行迁移（幂等）：按版本升序执行未应用项，事务内提交。
    ///
    /// `scope` 取 `"core"`（内核自身）或 `"module:<模块名>"`（模块）。
    pub fn migrate_scope(&self, scope: &str, migrations: &[Migration]) -> Result<(), KernelError> {
        let mut conn = self.conn.lock();
        let applied: Vec<u32> = {
            let mut stmt = conn.prepare(
                "SELECT version FROM schema_migrations WHERE scope = ?1 ORDER BY version",
            )?;
            let rows = stmt.query_map([scope], |r| r.get::<_, u32>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut pending: Vec<&Migration> = migrations
            .iter()
            .filter(|m| !applied.contains(&m.version))
            .collect();
        pending.sort_by_key(|m| m.version);

        for m in pending {
            let tx = conn.transaction()?;
            {
                tx.execute_batch(m.sql)?;
                tx.execute(
                    "INSERT INTO schema_migrations (scope, version, name) VALUES (?1, ?2, ?3)",
                    rusqlite::params![scope, m.version, m.name],
                )?;
            }
            tx.commit()?;
        }
        Ok(())
    }

    /// 当前已应用的最高版本（无则 0）。
    pub fn schema_version(&self, scope: &str) -> Result<u32, KernelError> {
        let conn = self.conn.lock();
        let v = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations WHERE scope = ?1",
                [scope],
                |r| r.get::<_, u32>(0),
            )
            .optional()?
            .unwrap_or(0);
        Ok(v)
    }

    /// 执行一段 SQL（多语句），用于内核初始化建表。
    pub fn exec_batch(&self, sql: &str) -> Result<(), KernelError> {
        let conn = self.conn.lock();
        conn.execute_batch(sql)?;
        Ok(())
    }

    /// 同步访问连接。`f` 内不得再调用本服务的锁方法，避免死锁。
    pub fn with_conn<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, KernelError>,
    ) -> Result<T, KernelError> {
        let mut conn = self.conn.lock();
        f(&mut conn)
    }
}

/// 内核自身 schema 迁移。模块的迁移由模块各自提供并调用 `migrate_scope`。
pub const CORE_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "core_settings",
        sql: "CREATE TABLE IF NOT EXISTS core_settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
              );
              CREATE TABLE IF NOT EXISTS core_account (
                id          TEXT PRIMARY KEY,
                gamertag    TEXT,
                xuid        TEXT,
                uuid        TEXT,
                ms_refresh_token TEXT,
                ms_access_token   TEXT,
                xbox_token  TEXT,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL
              );
              CREATE TABLE IF NOT EXISTS core_download_task (
                id         INTEGER PRIMARY KEY,
                url        TEXT NOT NULL,
                dest       TEXT NOT NULL,
                filename   TEXT,
                status     TEXT NOT NULL,
                total_bytes INTEGER NOT NULL DEFAULT 0,
                error      TEXT,
                created_at INTEGER NOT NULL
              );
              CREATE INDEX IF NOT EXISTS idx_download_task_created ON core_download_task(created_at);",
    },
];
