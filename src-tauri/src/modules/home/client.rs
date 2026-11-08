//! 第三方客户端库：导入、存储、选择与注入（开始页模块内部能力）。
//!
//! # 归属
//!
//! 客户端是**开始页模块**的功能，不是启动器级设置：
//! - 导入 / 命名 / 列表 / 删除的 UI 在实例的「管理客户端」页；
//! - 选择哪个客户端属于某个实例（VersionMeta.client_id + mode=client）；
//! - 只有 Windows 有实现；其它平台在 UI 与启动链路都拒绝。
//!
//! 客户端文件由启动器数据目录统一保管（<data>/clients/<id>/<文件名>），
//! 数据库记录名称、摘要与导入时间。实例只记客户端 id，不记绝对路径：路径属于
//! 启动器的数据布局，写进实例元数据会在迁移 / 重装后失效。
//!
//! 卸载启动器时由安装器的 NSIS 钩子删除客户端目录
//! （src-tauri/nsis-hooks.nsh，NSIS_HOOK_PREUNINSTALL）。
//!
//! # 注入时机
//!
//! 客户端不随游戏进程启动加载：游戏主模块与图形初始化尚未完成时，第三方客户端
//! （Latite 等）的挂钩会找不到目标符号。启动器在游戏进程起来后等待一段延迟
//! （每个客户端各自保存，默认 10 秒、可在「管理客户端」页调整）再通过远程线程
//! LoadLibraryW 注入，并在注入后核对模块表；失败时开始页提供「重新注入」入口。
//! 注入失败**不终止游戏**。
//!
//! # 平台
//!
//! 客户端库本身（DB / 文件 / 名称校验）平台中立，可在 Linux CI 上测；真正的
//! 进程注入只在 Windows 编译，命令层与 UI 都按平台拒绝。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
#[cfg(windows)]
use std::time::Duration;
use std::time::UNIX_EPOCH;

use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::KernelError;
use crate::registry::events::EventBus;
use crate::services::database::{DatabaseService, Migration};
use crate::services::paths::Paths;

/// 客户端库表迁移（作用域 module:home）。
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "module_home_client",
        sql: "CREATE TABLE IF NOT EXISTS module_home_client (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                file_name   TEXT NOT NULL,
                sha256      TEXT NOT NULL,
                size        INTEGER NOT NULL,
                imported_at INTEGER NOT NULL
              );
              CREATE UNIQUE INDEX IF NOT EXISTS idx_module_home_client_name
                ON module_home_client(name COLLATE NOCASE);
              CREATE UNIQUE INDEX IF NOT EXISTS idx_module_home_client_sha256
                ON module_home_client(sha256);",
    },
    Migration {
        version: 2,
        name: "module_home_client_inject_delay",
        sql: "ALTER TABLE module_home_client
                ADD COLUMN inject_delay_ms INTEGER NOT NULL DEFAULT 10000;",
    },
];

/// 客户端注入的默认等待时长（毫秒）。
///
/// 时机是客户端能否成功挂钩的硬约束，但不同客户端对初始化阶段的敏感度不同
/// （有的等主菜单、有的等图形设备就绪），所以延迟按客户端记录各自保存：
/// 默认 10 秒，允许在「管理客户端」页调整。
pub const DEFAULT_INJECTION_DELAY_MS: u64 = 10_000;
/// 注入延迟允许的上下界（毫秒）。
pub const MIN_INJECTION_DELAY_MS: u64 = 1_000;
pub const MAX_INJECTION_DELAY_MS: u64 = 120_000;

/// 单个客户端文件的大小上限（512 MiB）。超过这个体积的 DLL 不可能是客户端。
const MAX_CLIENT_BYTES: u64 = 512 * 1024 * 1024;
/// 显示名长度上限（按字符计）。
const MAX_NAME_CHARS: usize = 64;

/// 客户端视图（数据库记录 + 文件位置）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ClientView {
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub sha256: String,
    pub size: u64,
    pub imported_at: i64,
    /// 启动后等待多久再注入（毫秒）。按客户端保存，默认 10 秒。
    pub inject_delay_ms: u64,
    /// 客户端 DLL 的绝对路径（排障 / 打开目录用）。
    pub path: String,
    /// 文件是否仍在客户端目录里（被外部删掉时置 false）。
    pub available: bool,
}

/// 一次注入的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InjectResult {
    pub instance: String,
    pub client_id: String,
    pub client_name: String,
    pub success: bool,
    pub error: Option<String>,
    /// auto（启动后自动注入）或 manual（重新注入按钮）。
    pub source: String,
    /// 结果产生的 Unix 毫秒时间戳。
    pub at: i64,
}

/// 启动时的客户端注入计划（把校验过的路径带进后台任务）。
pub struct ClientLaunchPlan {
    pub client: ClientView,
    pub dll: PathBuf,
    pub delay_ms: u64,
}

/// 进程内的注入调度状态。
#[derive(Default)]
struct RuntimeState {
    /// 每个实例的调度代号：重新注入会让旧的自动任务失效。
    generation: HashMap<String, u64>,
    /// 正在注入的实例。
    injecting: HashSet<String>,
    /// 最近一次注入结果。
    last: HashMap<String, InjectResult>,
}

fn runtime() -> &'static Mutex<RuntimeState> {
    static STATE: std::sync::OnceLock<Mutex<RuntimeState>> = std::sync::OnceLock::new();
    STATE.get_or_init(|| Mutex::new(RuntimeState::default()))
}

/// 客户端文件根目录：<数据目录>/clients。
pub fn clients_dir(paths: &Paths) -> PathBuf {
    paths.data_dir().join("clients")
}

/// 当前构建是否支持客户端注入（仅 Windows）。
pub fn supported() -> bool {
    cfg!(windows)
}

/// 命令层入口的平台闸门：非 Windows 一律拒绝，与 UI 隐藏双保险。
pub fn ensure_supported() -> Result<(), KernelError> {
    if supported() {
        Ok(())
    } else {
        Err(KernelError::InvalidArgument(
            "客户端注入仅支持 Windows".into(),
        ))
    }
}

/// 校验显示名并返回去空白后的值。
pub fn validate_name(raw: &str) -> Result<String, KernelError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(KernelError::InvalidArgument("客户端名称不能为空".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(KernelError::InvalidArgument(format!(
            "客户端名称过长（最多 {MAX_NAME_CHARS} 字符）"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(KernelError::InvalidArgument(
            "客户端名称不能包含控制字符".into(),
        ));
    }
    Ok(name.to_string())
}

/// 把源文件名规整成可以安全落盘的文件名。
///
/// 保留原文件名是为了让依赖查找与日志排障更直观；只替换 Windows 不允许的字符，
/// 并强制 .dll 后缀与长度上限。规整后为空时回落 client.dll。
fn sanitize_file_name(source: &Path) -> String {
    let raw = source
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut cleaned = String::with_capacity(raw.len());
    let mut last_underscore = false;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ' ' | '(' | ')') {
            ch
        } else {
            '_'
        };
        if mapped == '_' {
            if last_underscore {
                continue;
            }
            last_underscore = true;
        } else {
            last_underscore = false;
        }
        cleaned.push(mapped);
    }
    let mut cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        cleaned = "client.dll".to_string();
    }
    if !cleaned.to_ascii_lowercase().ends_with(".dll") {
        cleaned.push_str(".dll");
    }
    // 文件名长度按字符截断到 120，避免超过 NTFS 的 255 字符上限（目录名另算）。
    let take = cleaned.chars().count().min(120);
    cleaned.chars().take(take).collect()
}

/// 校验客户端 id 是否可以安全拼进路径。
fn valid_client_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn row_to_view(paths: &Paths, row: &rusqlite::Row<'_>) -> Result<ClientView, rusqlite::Error> {
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let file_name: String = row.get(2)?;
    let sha256: String = row.get(3)?;
    let size: i64 = row.get(4)?;
    let imported_at: i64 = row.get(5)?;
    let inject_delay_ms: i64 = row.get(6)?;
    let path = client_path(paths, &id, &file_name);
    Ok(ClientView {
        id,
        name,
        file_name,
        sha256,
        size: size.max(0) as u64,
        imported_at,
        inject_delay_ms: inject_delay_ms.max(0) as u64,
        available: path.is_file(),
        path: path.to_string_lossy().into_owned(),
    })
}

fn client_path(paths: &Paths, id: &str, file_name: &str) -> PathBuf {
    clients_dir(paths).join(id).join(file_name)
}

/// 读取全部客户端记录（按导入时间倒序）。
pub fn list(paths: &Paths, db: &DatabaseService) -> Result<Vec<ClientView>, KernelError> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, file_name, sha256, size, imported_at, inject_delay_ms
               FROM module_home_client
              ORDER BY imported_at DESC, name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| row_to_view(paths, row))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
}

/// 按 id 读取单个客户端记录（不存在返回 None）。
pub fn get(
    paths: &Paths,
    db: &DatabaseService,
    id: &str,
) -> Result<Option<ClientView>, KernelError> {
    if !valid_client_id(id) {
        return Ok(None);
    }
    db.with_conn(|conn| {
        Ok(conn
            .query_row(
                "SELECT id, name, file_name, sha256, size, imported_at, inject_delay_ms
                   FROM module_home_client WHERE id = ?1",
                [id],
                |row| row_to_view(paths, row),
            )
            .ok())
    })
}

fn publish_changed(events: &EventBus) {
    events.publish("clients.changed", serde_json::json!({}));
}

/// 调整某个客户端的注入延迟（毫秒）。
///
/// 延迟按客户端保存而不是按实例：同一个 DLL 在不同实例里的初始化节奏没有差别，
/// 按实例存只会让人误以为「换个实例设置就丢了」。
pub fn set_inject_delay(
    paths: &Paths,
    db: &DatabaseService,
    events: &EventBus,
    id: &str,
    delay_ms: u64,
) -> Result<ClientView, KernelError> {
    if !(MIN_INJECTION_DELAY_MS..=MAX_INJECTION_DELAY_MS).contains(&delay_ms) {
        return Err(KernelError::InvalidArgument(format!(
            "注入时机需要在 {} 到 {} 秒之间",
            MIN_INJECTION_DELAY_MS / 1000,
            MAX_INJECTION_DELAY_MS / 1000
        )));
    }
    if get(paths, db, id)?.is_none() {
        return Err(KernelError::InvalidArgument("客户端不存在".into()));
    }
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE module_home_client SET inject_delay_ms = ?1 WHERE id = ?2",
            rusqlite::params![delay_ms as i64, id],
        )?;
        Ok(())
    })?;
    publish_changed(events);
    get(paths, db, id)?.ok_or_else(|| KernelError::Database(rusqlite::Error::QueryReturnedNoRows))
}

/// 导入一个客户端 DLL。
///
/// 校验顺序：名称 → 源文件 → 体积 → 摘要 → x64 → 重名 / 重复文件。任何一步
/// 失败都不落盘、不写库。落盘使用「临时文件 + rename」，避免半个 DLL 被当成
/// 可用客户端。
pub fn import(
    paths: &Paths,
    db: &DatabaseService,
    events: &EventBus,
    source_path: &str,
    raw_name: &str,
) -> Result<ClientView, KernelError> {
    let name = validate_name(raw_name)?;
    let source = PathBuf::from(source_path.trim());
    if source_path.trim().is_empty() || !source.is_file() {
        return Err(KernelError::InvalidArgument(
            "选择的客户端文件不存在".into(),
        ));
    }
    let extension = source
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if extension != "dll" {
        return Err(KernelError::InvalidArgument(
            "客户端必须是 DLL 文件".into(),
        ));
    }
    let metadata = std::fs::metadata(&source)?;
    if metadata.len() == 0 || metadata.len() > MAX_CLIENT_BYTES {
        return Err(KernelError::InvalidArgument(
            "客户端文件体积异常（空文件或超过 512 MB）".into(),
        ));
    }

    let bytes = std::fs::read(&source)?;
    let digest = sha256_hex(&bytes);
    // 架构校验：x86 / ARM64 的 DLL 注入 x64 游戏只会在 LoadLibrary 时失败，
    // 与其让用户在游戏里遇到一个没有解释的错误，不如导入时就拒绝。
    let image = crate::pe::parse(bytes.clone())
        .map_err(|error| KernelError::InvalidArgument(format!("客户端不是有效的 DLL：{error}")))?;
    if !image.is_x64() {
        return Err(KernelError::InvalidArgument(
            "客户端必须是 x64 DLL（游戏是 64 位进程）".into(),
        ));
    }

    let id = Uuid::new_v4().to_string();
    let file_name = sanitize_file_name(&source);

    // 重名 / 重复文件在写盘前检查：唯一索引只是最后一道保险。
    db.with_conn(|conn| {
        if let Some(existing) = conn
            .query_row(
                "SELECT name FROM module_home_client WHERE name = ?1 COLLATE NOCASE",
                [&name],
                |row| row.get::<_, String>(0),
            )
            .ok()
        {
            return Err(KernelError::Conflict(format!(
                "已存在同名客户端「{existing}」"
            )));
        }
        if let Some(existing) = conn
            .query_row(
                "SELECT name FROM module_home_client WHERE sha256 = ?1",
                [&digest],
                |row| row.get::<_, String>(0),
            )
            .ok()
        {
            return Err(KernelError::Conflict(format!(
                "该客户端已导入过（名称「{existing}」）"
            )));
        }
        Ok(())
    })?;

    let dir = clients_dir(paths).join(&id);
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(&file_name);
    let tmp = target.with_extension("dll.copper-tmp");
    let write_result = (|| -> Result<(), KernelError> {
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &target)?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_dir_all(&dir);
        return Err(error);
    }

    let imported_at = now_millis();
    let insert = db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO module_home_client (id, name, file_name, sha256, size, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![id, name, file_name, digest, bytes.len() as i64, imported_at],
        )?;
        Ok(())
    });
    if let Err(error) = insert {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(error);
    }

    publish_changed(events);
    get(paths, db, &id)?
        .ok_or_else(|| KernelError::Database(rusqlite::Error::QueryReturnedNoRows))
}

/// 删除客户端（数据库记录 + 文件目录）。
pub fn remove(
    paths: &Paths,
    db: &DatabaseService,
    events: &EventBus,
    id: &str,
) -> Result<(), KernelError> {
    if !valid_client_id(id) {
        return Err(KernelError::InvalidArgument("客户端标识非法".into()));
    }
    let Some(view) = get(paths, db, id)? else {
        return Err(KernelError::InvalidArgument("客户端不存在".into()));
    };

    let dir = clients_dir(paths).join(&view.id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|error| {
            KernelError::InvalidArgument(format!(
                "删除客户端文件失败（游戏可能正在使用该客户端，请先退出游戏）：{error}"
            ))
        })?;
    }
    db.with_conn(|conn| {
        conn.execute("DELETE FROM module_home_client WHERE id = ?1", [id])?;
        Ok(())
    })?;
    publish_changed(events);
    Ok(())
}

/// 构造某个实例的启动注入计划。
///
/// 只有实例模式为 client 时才返回计划；此时 client_id 必须指向一个可用客户端。
/// 注入前重新核对摘要：客户端文件被外部替换时必须拒绝，而不是加载一个来路不明的 DLL。
pub fn launch_plan(
    paths: &Paths,
    db: &DatabaseService,
    mode: Option<&str>,
    client_id: Option<&str>,
) -> Result<Option<ClientLaunchPlan>, KernelError> {
    if mode != Some("client") {
        return Ok(None);
    }
    let id = client_id.unwrap_or("").trim();
    if id.is_empty() {
        return Err(KernelError::InvalidArgument(
            "客户端模式尚未选择客户端，请在实例的「管理客户端」页选择".into(),
        ));
    }
    let client = get(paths, db, id)?
        .ok_or_else(|| KernelError::InvalidArgument("选中的客户端记录不存在，请重新选择".into()))?;
    if !client.available {
        return Err(KernelError::InvalidArgument(format!(
            "客户端文件已丢失：{}",
            client.name
        )));
    }
    let dll = PathBuf::from(&client.path);
    let bytes = std::fs::read(&dll)?;
    if sha256_hex(&bytes) != client.sha256 {
        return Err(KernelError::InvalidArgument(format!(
            "客户端文件校验失败：{}（文件可能被修改，请重新导入）",
            client.name
        )));
    }
    let delay_ms = client.inject_delay_ms;
    Ok(Some(ClientLaunchPlan {
        client,
        dll,
        delay_ms,
    }))
}

#[cfg(windows)]
fn next_generation(instance: &str) -> u64 {
    let mut state = runtime().lock();
    let entry = state.generation.entry(instance.to_string()).or_insert(0);
    *entry = entry.wrapping_add(1);
    *entry
}

#[cfg(windows)]
fn generation_is(instance: &str, expected: u64) -> bool {
    runtime()
        .lock()
        .generation
        .get(instance)
        .map(|value| *value == expected)
        .unwrap_or(false)
}

/// 启动后按客户端配置的延迟自动注入（后台线程，不阻塞启动命令）。
#[cfg(windows)]
pub fn schedule_injection(
    events: Arc<EventBus>,
    instance: String,
    pid: u32,
    plan: ClientLaunchPlan,
) {
    let generation = next_generation(&instance);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(plan.delay_ms));
        if !generation_is(&instance, generation) {
            log::info!("[home/client] 实例 {instance} 的自动注入已被重新注入取代");
            return;
        }
        perform_injection(events.as_ref(), &instance, pid, &plan.client, &plan.dll, "auto");
    });
}

/// 非 Windows：没有进程注入，不排任何任务。
#[cfg(not(windows))]
pub fn schedule_injection(
    _events: Arc<EventBus>,
    _instance: String,
    _pid: u32,
    _plan: ClientLaunchPlan,
) {
    log::warn!("[home/client] 当前平台不支持客户端注入，已跳过");
}

/// 手动「重新注入」：取消尚未执行的自动注入，立即注入一次。
///
/// 返回时注入任务已投递，结果经 client.inject_result 事件回传；命令本身不等待
/// 远程线程，避免按钮卡住十几秒。
#[cfg(windows)]
pub fn reinject(
    paths: &Paths,
    db: &DatabaseService,
    events: Arc<EventBus>,
    instance: &str,
    versions_root: &Path,
    mode: Option<&str>,
    client_id: Option<&str>,
) -> Result<(), KernelError> {
    let plan = launch_plan(paths, db, mode, client_id)?
        .ok_or_else(|| KernelError::InvalidArgument("当前实例不是客户端模式".into()))?;
    if runtime().lock().injecting.contains(instance) {
        return Err(KernelError::Conflict("正在注入中，请稍候".into()));
    }
    let dir = crate::modules::home::meta::resolve_version_dir(versions_root, instance)?;
    let exe = crate::modules::home::launch::game_exe_path(&dir);
    let pid = crate::modules::home::launch::find_pid_at_path(&exe)
        .ok_or_else(|| KernelError::InvalidArgument("游戏未运行，无法注入客户端".into()))?;
    // 让仍在等待的自动任务失效，避免 5s 后重复注入同一个 DLL。
    next_generation(instance);
    let instance = instance.to_string();
    std::thread::spawn(move || {
        perform_injection(
            events.as_ref(),
            &instance,
            pid,
            &plan.client,
            &plan.dll,
            "manual",
        );
    });
    Ok(())
}

#[cfg(not(windows))]
pub fn reinject(
    _paths: &Paths,
    _db: &DatabaseService,
    _events: Arc<EventBus>,
    _instance: &str,
    _versions_root: &Path,
    _mode: Option<&str>,
    _client_id: Option<&str>,
) -> Result<(), KernelError> {
    Err(KernelError::InvalidArgument(
        "客户端注入仅支持 Windows".into(),
    ))
}

#[cfg(windows)]
fn perform_injection(
    events: &EventBus,
    instance: &str,
    pid: u32,
    client: &ClientView,
    dll: &Path,
    source: &str,
) {
    {
        let mut state = runtime().lock();
        if !state.injecting.insert(instance.to_string()) {
            return;
        }
    }
    log::info!(
        "[home/client] 开始注入客户端 '{}' 到实例 {instance}（pid={pid}, source={source}）",
        client.name
    );
    events.publish(
        "client.inject_started",
        serde_json::json!({
            "instance": instance,
            "client_id": client.id,
            "client_name": client.name,
            "source": source,
        }),
    );

    let outcome = crate::modules::home::injector::inject_dll_into_process(pid, dll).and_then(|()| {
        // LoadLibrary 返回非零只代表调用成功；再核对模块表，确认 DLL 真的在
        // 目标进程里。客户端被安全软件「放行调用、拦截加载」时就差这一层。
        match crate::modules::home::injector::is_module_loaded(pid, &client.file_name) {
            Some(true) => Ok(()),
            Some(false) => Err(KernelError::Module(format!(
                "远程线程已返回，但模块 '{}' 未出现在游戏进程模块表中",
                client.file_name
            ))),
            None => {
                // 快照不可用（权限 / 进程刚退出）时不能把「无法核对」当成失败：
                // LoadLibrary 已返回非零，先按成功上报，并在日志里保留这一情况。
                log::warn!(
                    "[home/client] 无法读取 pid={pid} 的模块表，跳过模块核对（按 LoadLibrary 返回值判定成功）"
                );
                Ok(())
            }
        }
    });

    let result = InjectResult {
        instance: instance.to_string(),
        client_id: client.id.clone(),
        client_name: client.name.clone(),
        success: outcome.is_ok(),
        error: outcome.as_ref().err().map(|error| error.payload()),
        source: source.to_string(),
        at: now_millis(),
    };
    match &outcome {
        Ok(()) => log::info!(
            "[home/client] 客户端 '{}' 已注入实例 {instance}",
            client.name
        ),
        Err(error) => log::error!(
            "[home/client] 客户端 '{}' 注入实例 {instance} 失败：{error}",
            client.name
        ),
    }
    {
        let mut state = runtime().lock();
        state.injecting.remove(instance);
        state.last.insert(instance.to_string(), result.clone());
    }
    events.publish(
        "client.inject_result",
        serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "copper_client_{tag}_{}_{}",
            std::process::id(),
            now_millis()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data")).unwrap();
        root
    }

    /// 最小 PE32+ x64 镜像：只满足 pe::parse 的结构要求。
    fn minimal_dll() -> Vec<u8> {
        let mut data = vec![0u8; 0x400];
        data[0..2].copy_from_slice(b"MZ");
        data[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        let pe = 0x80usize;
        data[pe..pe + 4].copy_from_slice(b"PE\0\0");
        let coff = pe + 4;
        data[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
        data[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
        data[coff + 16..coff + 18].copy_from_slice(&0xf0u16.to_le_bytes());
        let opt = coff + 20;
        data[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        data[opt + 32..opt + 36].copy_from_slice(&0x1000u32.to_le_bytes());
        data[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes());
        data[opt + 56..opt + 60].copy_from_slice(&0x2000u32.to_le_bytes());
        data[opt + 60..opt + 64].copy_from_slice(&0x400u32.to_le_bytes());
        data[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
        // 节表在 optional header 之后。
        let section = opt + 0xf0;
        data[section..section + 5].copy_from_slice(b".text");
        data[section + 8..section + 12].copy_from_slice(&0x10u32.to_le_bytes());
        data[section + 12..section + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        data[section + 16..section + 20].copy_from_slice(&0x200u32.to_le_bytes());
        data[section + 20..section + 24].copy_from_slice(&0x400u32.to_le_bytes());
        data[section + 36..section + 40].copy_from_slice(&0x40000040u32.to_le_bytes());
        data
    }

    fn services(root: &Path) -> (Arc<DatabaseService>, Arc<EventBus>, Arc<Paths>) {
        let paths = Arc::new(Paths::with_root(root.to_path_buf()));
        let db = Arc::new(DatabaseService::open(&root.join("data/test.db")).unwrap());
        db.migrate_scope("module:home", MIGRATIONS).unwrap();
        let events = Arc::new(EventBus::new());
        (db, events, paths)
    }

    #[test]
    fn name_validation_rejects_empty_and_control() {
        assert!(validate_name("  ").is_err());
        assert!(validate_name("Latite 26.4").is_ok());
        assert!(validate_name(&"a".repeat(65)).is_err());
        assert!(validate_name("a\u{7}b").is_err());
    }

    #[test]
    fn file_name_is_sanitized_but_keeps_dll_suffix() {
        assert_eq!(sanitize_file_name(Path::new("D:/x/Latite.dll")), "Latite.dll");
        assert_eq!(sanitize_file_name(Path::new("D:/x/bad*name")), "bad_name.dll");
        assert_eq!(sanitize_file_name(Path::new("")), "client.dll");
        assert!(!sanitize_file_name(Path::new("x/..")).contains('/'));
    }

    #[test]
    fn import_requires_a_real_x64_dll_and_deduplicates() {
        let root = temp_root("import");
        let (db, events, paths) = services(&root);
        let source = root.join("Latite.dll");
        std::fs::write(&source, minimal_dll()).unwrap();

        let view = import(
            &paths,
            &db,
            &events,
            &source.to_string_lossy(),
            "Latite",
        )
        .expect("合法 x64 DLL 应可导入");
        assert_eq!(view.name, "Latite");
        assert!(view.available);
        assert_eq!(view.sha256.len(), 64);
        assert!(Path::new(&view.path).is_file());
        assert_eq!(get(&paths, &db, &view.id).unwrap().unwrap().id, view.id);

        // 同名 / 同文件都拒绝。
        assert!(import(
            &paths,
            &db,
            &events,
            &source.to_string_lossy(),
            "Latite"
        )
        .unwrap_err()
        .to_string()
        .contains("同名"));
        assert!(import(
            &paths,
            &db,
            &events,
            &source.to_string_lossy(),
            "Latite 2"
        )
        .is_err());

        // 非 DLL / 非 x64 拒绝。
        let txt = root.join("note.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(import(&paths, &db, &events, &txt.to_string_lossy(), "x").is_err());
        let mut arm = minimal_dll();
        arm[0x84..0x86].copy_from_slice(&0xaa64u16.to_le_bytes());
        let arm_path = root.join("arm.dll");
        std::fs::write(&arm_path, arm).unwrap();
        assert!(import(
            &paths,
            &db,
            &events,
            &arm_path.to_string_lossy(),
            "arm"
        )
        .is_err());

        // 删除后记录与目录都清空。
        remove(&paths, &db, &events, &view.id).unwrap();
        assert!(list(&paths, &db).unwrap().is_empty());
        assert!(!Path::new(&view.path).exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn launch_plan_requires_client_mode_and_a_valid_client() {
        let root = temp_root("plan");
        let (db, events, paths) = services(&root);
        let source = root.join("Latite.dll");
        std::fs::write(&source, minimal_dll()).unwrap();
        let view = import(
            &paths,
            &db,
            &events,
            &source.to_string_lossy(),
            "Latite",
        )
        .unwrap();

        // 非客户端模式不产生注入计划。
        assert!(launch_plan(&paths, &db, None, None).unwrap().is_none());
        assert!(launch_plan(&paths, &db, Some("original"), Some(&view.id))
            .unwrap()
            .is_none());
        // 客户端模式但没选客户端 / 记录不存在：明确报错。
        assert!(launch_plan(&paths, &db, Some("client"), None).is_err());
        assert!(launch_plan(&paths, &db, Some("client"), Some("missing")).is_err());

        let plan = launch_plan(&paths, &db, Some("client"), Some(&view.id))
            .unwrap()
            .unwrap();
        assert_eq!(plan.client.id, view.id);
        assert_eq!(view.inject_delay_ms, DEFAULT_INJECTION_DELAY_MS);
        assert_eq!(plan.delay_ms, DEFAULT_INJECTION_DELAY_MS);

        // 注入延迟按客户端保存，越界值被拒绝。
        assert!(set_inject_delay(&paths, &db, &events, &view.id, 500).is_err());
        assert!(set_inject_delay(&paths, &db, &events, &view.id, 600_000).is_err());
        let adjusted = set_inject_delay(&paths, &db, &events, &view.id, 15_000).unwrap();
        assert_eq!(adjusted.inject_delay_ms, 15_000);
        let plan = launch_plan(&paths, &db, Some("client"), Some(&view.id))
            .unwrap()
            .unwrap();
        assert_eq!(plan.delay_ms, 15_000);

        // 文件被外部删掉：计划拒绝，而不是加载不存在的路径。
        std::fs::remove_file(&view.path).unwrap();
        assert!(launch_plan(&paths, &db, Some("client"), Some(&view.id)).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn launch_plan_rejects_a_modified_client_file() {
        let root = temp_root("tamper");
        let (db, events, paths) = services(&root);
        let source = root.join("Latite.dll");
        std::fs::write(&source, minimal_dll()).unwrap();
        let view = import(
            &paths,
            &db,
            &events,
            &source.to_string_lossy(),
            "Latite",
        )
        .unwrap();
        std::fs::write(&view.path, minimal_dll()[..0x300].to_vec()).unwrap();
        assert!(launch_plan(&paths, &db, Some("client"), Some(&view.id)).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
