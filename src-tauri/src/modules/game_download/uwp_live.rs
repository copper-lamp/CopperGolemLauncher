//! UWP 端到端验收（默认 `#[ignore]`，需联网 + Windows 开发者模式）。
//!
//! 两条互相独立的真机验证：
//!
//! ```text
//! # 1) 从 Windows Update 真下载一个 UWP 整包（约 900 MB），验证地址解析与官方摘要
//! cargo test -p copper-core --lib live_uwp_downloads_from_windows_update -- --ignored --nocapture
//!
//! # 2) 安装 → 实例落盘 → 启动 → 隔离生效（复用上一步下载的整包，不重复下载）
//! cargo test -p copper-core --lib live_uwp_install_and_launch -- --ignored --nocapture
//! ```
//!
//! 两者都用 `COPPER_UWP_LIVE_ROOT` 指向同一个临时版本根（默认
//! `CopperCore/.devdata/uwp-live`），第二个用例因此可以直接复用第一个用例下载的
//! `.download/1.21.114.1.appx`——但摘要仍会重新校验一遍（那是安装的硬前提）。
//!
//! 为什么用 `1.21.114.1`：它是 UWP 渠道最后一个正式版（之后的 1.21.120.04 起改由
//! GDK 分发），也是本条链路在实机上唯一验证过的版本。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::KernelError;
use crate::modules::home::{launch, meta::VersionMeta};
use crate::registry::events::EventBus;
use crate::services::account::AccountService;
use crate::services::database::DatabaseService;
use crate::services::download::DownloadService;
use crate::services::paths::Paths;
use crate::services::settings::SettingsService;

use super::installer::{self, Ctx};

/// 验收用的 UWP 版本（UWP 渠道最后一个正式版）。
const LIVE_VERSION: &str = "1.21.114.1";

fn live_root() -> std::path::PathBuf {
    std::env::var("COPPER_UWP_LIVE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(".devdata")
                .join("uwp-live")
        })
}

/// 每个用例一个干净的子根：数据库记录与 `.download` 里的整包都不跨用例共享，
/// 用例因此可以重复运行（否则第二次跑会因为「本地已有整包」直接返回 0 任务）。
fn fresh_root(tag: &str) -> std::path::PathBuf {
    let root = live_root().join(tag);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("创建测试版本根");
    root
}

/// 组装安装流水线上下文（与内核里的 `Ctx::from_kernel` 等价，只是服务由测试自建）。
fn build_ctx(root: &std::path::Path) -> (Ctx, Arc<EventBus>, tokio::runtime::Runtime) {
    std::fs::create_dir_all(root).expect("创建测试版本根");
    // 版本根由 `COPPER_VERSIONS_DIR` 覆盖：与 dev 会话用同一条解析入口。
    std::env::set_var("COPPER_VERSIONS_DIR", root.join("versions"));
    let paths = Arc::new(Paths::with_root(root.to_path_buf()));
    let runtime = tokio::runtime::Runtime::new().expect("创建 tokio 运行时");
    let events = Arc::new(EventBus::new());
    let db = Arc::new(DatabaseService::open(&paths.db_file()).expect("打开测试数据库"));
    db.migrate_scope("core", crate::services::database::CORE_MIGRATIONS)
        .expect("迁移 core schema");
    db.migrate_scope("module:game-download", installer::MIGRATIONS)
        .expect("迁移 game-download schema");
    let settings = Arc::new(
        SettingsService::new(db.clone(), events.clone(), Default::default())
            .expect("创建设置服务"),
    );
    let download = Arc::new(DownloadService::new(
        2,
        runtime.handle().clone(),
        paths.clone(),
        db.clone(),
        events.clone(),
    ));
    let account = Arc::new(AccountService::new(
        db.clone(),
        events.clone(),
        crate::platform::as_secret(&crate::platform::Backends::assemble()),
        runtime.handle().clone(),
    ));
    let ctx = Ctx {
        events: events.clone(),
        db,
        paths,
        settings,
        download,
        account,
        store_http: Arc::new(
            crate::services::http_client::client_builder(Duration::from_secs(40))
                .build()
                .expect("构建 HTTP 客户端"),
        ),
        runtime: runtime.handle().clone(),
        window: Arc::new(crate::services::window::MainWindow::new()),
    };
    (ctx, events, runtime)
}

/// 订阅 `download.status` 驱动安装（等价于模块 `init()` 里的那条订阅）。
fn subscribe_install(ctx: &Ctx, events: &Arc<EventBus>) {
    let sub_ctx = ctx.clone();
    let lock = installer::install_lock();
    // 订阅句柄是 Copy 的 id，丢弃不会退订（退订要走 events.unsubscribe）；
    // 测试进程即将退出，也不需要显式退订。
    let _ = events.subscribe("download.status", move |_name, payload: &Value| {
        installer::handle_status(&sub_ctx, &lock, payload);
    });
}

/// 测试收尾：tokio runtime 的 `drop` 会等阻塞线程池清空，而下载服务带常驻线程，
/// 于是进程会卡在用例结束之后。手动运行时在这里直接 forget（进程随即退出）。
fn dispose(runtime: tokio::runtime::Runtime) {
    std::mem::forget(runtime);
}

fn wait_until(timeout: Duration, mut probe: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if probe() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    probe()
}

#[test]
#[ignore = "真联网：从 Windows Update 下载约 900 MB 的 UWP 整包"]
fn live_uwp_downloads_from_windows_update() {
    let root = fresh_root("download");
    let (ctx, _events, runtime) = build_ctx(&root);

    let task_id = runtime
        .block_on(installer::enqueue(
            &ctx,
            &installer::install_lock(),
            LIVE_VERSION,
            "uwp-download-probe",
            None,
        ))
        .expect("UWP 入队必须成功（地址解析 / 摘要获取都在这一步）");
    println!("[live-uwp] 下载任务 id = {task_id}");
    assert!(task_id > 0, "首次下载必须真的投递一个任务（本地没有整包）");

    // 等下载落盘：`.download/<slug>.appx` 出现即视为完成（引擎 Done 事件再触发安装）。
    let dest = ctx.dest_for_uwp(LIVE_VERSION);
    let ok = wait_until(Duration::from_secs(1800), || {
        dest.is_file() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false)
    });
    let size = dest.metadata().map(|m| m.len()).unwrap_or(0);
    println!("[live-uwp] 整包 {} 字节 -> {}", size, dest.display());
    assert!(ok && size > 100 * 1024 * 1024, "整包必须落盘且体积合理");

    // 官方摘要校验（解析出来的 digest 存在数据库记录上）。
    let record = ctx
        .db
        .with_conn(|conn| -> Result<Option<(String, String)>, KernelError> {
            let mut stmt = conn.prepare(
                "SELECT package_type, digest FROM module_game_download_package WHERE version_id = ?1",
            )?;
            let mut rows = stmt.query_map([LIVE_VERSION], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            match rows.next() {
                Some(row) => Ok(Some(row?)),
                None => Ok(None),
            }
        })
        .expect("读取整包记录")
        .expect("必须有整包记录");
    println!("[live-uwp] 记录: 类型={} 摘要={}", record.0, record.1);
    assert_eq!(record.0, "uwp");
    assert!(record.1.starts_with("sha1:") || record.1.starts_with("sha256:"));
    assert!(
        super::uwp_source::verify_digest(&dest, &record.1, &mut |_, _| {}).expect("摘要计算"),
        "下载下来的整包必须与 Windows Update 下发的摘要一致"
    );
    println!("[live-uwp] 摘要校验通过");
    dispose(runtime);
}

#[test]
#[ignore = "真联网：UWP 安装落盘（不启动游戏）"]
fn live_uwp_installs_instance() {
    let root = fresh_root("install");
    let (ctx, events, runtime) = build_ctx(&root);
    subscribe_install(&ctx, &events);

    let versions_root = ctx.versions_root();
    let instance = "uwp-live-isolation";
    let _ = std::fs::remove_dir_all(versions_root.join(instance));

    let task_id = runtime
        .block_on(installer::enqueue(
            &ctx,
            &installer::install_lock(),
            LIVE_VERSION,
            instance,
            None,
        ))
        .expect("UWP 入队必须成功");
    println!("[live-uwp] 任务 id = {task_id}（0 表示复用本地整包）");

    let instance_dir = ctx.install_dir(instance);
    let installed = wait_until(Duration::from_secs(1800), || {
        VersionMeta::read(&instance_dir).is_some()
    });
    let meta = VersionMeta::read(&instance_dir).expect("安装完成后必须能读到 version.json");
    println!(
        "[live-uwp] 实例: {} / {} / packageType={:?} / uwp={:?}",
        meta.name, meta.game_version, meta.package_type, meta.uwp
    );
    assert!(installed, "安装必须在超时前完成");
    assert!(meta.is_uwp(), "version.json 必须标记为 UWP 实例");
    assert!(meta.uwp_identity().is_some(), "必须写入完整包身份");
    assert!(
        meta.game_version == LIVE_VERSION,
        "gameVersion 必须是清单换算值（1.21.11401.0 -> 1.21.114.1），实际 {}",
        meta.game_version
    );
    assert!(instance_dir.join("AppxManifest.xml").is_file());
    assert!(instance_dir.join("Minecraft.Windows.exe").is_file());
    // 包签名不能留在松散目录里（留着会让系统拒绝松散注册）。
    assert!(!instance_dir.join("AppxSignature.p7x").exists());
    // 加载器字段必须为空：UWP 实例不支持 LeviLamina。
    assert!(meta.loader.is_none(), "UWP 实例不应带加载器: {:?}", meta.loader);

    // 开始页扫描口径：`home::meta::scan_versions` 必须能看到它。
    let scanned = crate::modules::home::meta::scan_versions(&versions_root);
    assert!(
        scanned.iter().any(|item| item.name == instance),
        "开始页扫描必须收录该实例"
    );

    // 内容根（不启动游戏也要能解析出正确落点，内容下载依赖它）。
    let data_dir = crate::modules::home::isolate::data_dir(&instance_dir, &meta);
    println!("[live-uwp] 隔离数据根 = {}", data_dir.display());
    assert_eq!(
        data_dir,
        instance_dir.join("Minecraft Bedrock"),
        "正式版 UWP 实例的数据根必须是 `Minecraft Bedrock`"
    );
    let roots = crate::modules::home::isolate::content_roots(&instance_dir, &meta);
    assert_eq!(
        roots.com_mojang,
        data_dir.join("games").join("com.mojang")
    );
    assert!(
        roots.worlds.as_ref().is_some_and(|worlds| worlds.ends_with("minecraftWorlds")),
        "UWP 内容根必须给出确定的世界目录"
    );

    println!("[live-uwp] 安装完成，实例目录 = {}", instance_dir.display());
    dispose(runtime);
}

/// 启动 + 隔离验收（独立用例，与安装分开：安装只依赖网络与本机解包，启动还依赖
/// Windows 开发者模式下的松散注册与系统激活）。
///
/// 复用 `live_uwp_installs_instance` 留下的实例目录，不重复下载 / 解包；
/// 实例不存在时先跑安装。
#[test]
#[ignore = "真启动游戏：UWP 系统激活与实例数据隔离验收"]
fn live_uwp_launch_and_isolation() {
    let root = live_root().join("install");
    assert!(
        root.is_dir(),
        "请先运行 live_uwp_installs_instance（缺少安装产物 {}）",
        root.display()
    );
    let (ctx, events, runtime) = build_ctx(&root);
    let instance = "uwp-live-isolation";
    let instance_dir = ctx.install_dir(instance);
    let meta = VersionMeta::read(&instance_dir).expect("实例必须已经安装好");

    // ---- 启动 + 隔离验收 ----
    let paths = Arc::new(Paths::with_root(root.clone()));
    let db = Arc::new(DatabaseService::open(&paths.db_file()).expect("打开测试数据库"));
    let settings = Arc::new(
        SettingsService::new(db.clone(), events.clone(), Default::default())
            .expect("创建设置服务"),
    );
    let launch_ctx = launch::LaunchCtx {
        paths,
        settings,
        db,
        runtime: runtime.handle().clone(),
        events: events.clone(),
        account: None,
        hwnd: 0,
    };
    launch::launch_game(&launch_ctx, instance, true).expect("UWP 启动链路必须成功");

    let exe = launch::game_exe_path(&instance_dir);
    let pid = launch::find_pid_at_path(&exe).unwrap_or(0);
    println!("[live-uwp] 游戏 pid = {pid}");
    assert!(pid != 0, "激活后必须能按 exe 路径找到游戏进程");

    let data_dir = crate::modules::home::isolate::data_dir(&instance_dir, &meta);
    let games = data_dir.join("games").join("com.mojang");
    let arrived = wait_until(Duration::from_secs(120), || {
        games.read_dir().map(|mut dir| dir.next().is_some()).unwrap_or(false)
    });
    let count = walk(&games);
    println!("[live-uwp] 实例数据目录 {} 个文件: {}", count, games.display());
    assert!(
        arrived,
        "游戏必须把数据写进实例目录（隔离不生效时这里会是空的）"
    );
    assert!(
        games.join("minecraftpe").is_dir() || games.join("minecraftWorlds").is_dir(),
        "实例目录里必须出现游戏自己的数据子目录"
    );

    // 系统侧确认：包注册指向的就是这个实例目录。
    let identity = meta.uwp_identity().expect("包身份");
    let registered = super::uwp_deploy::query(&identity.identity_name)
        .expect("查询包注册")
        .expect("启动后包必须处于已注册状态");
    println!(
        "[live-uwp] 注册: {} @ {}",
        registered.full_name,
        registered.install_location.display()
    );
    assert_eq!(
        std::fs::canonicalize(&registered.install_location).unwrap_or(registered.install_location.clone()),
        std::fs::canonicalize(&instance_dir).unwrap_or(instance_dir.clone()),
        "包注册必须指向该实例目录"
    );

    // 收尾：结束游戏进程，避免测试留下一个前台窗口。
    let killed = launch::terminate_process_at_path(&exe).unwrap_or(0);
    println!("[live-uwp] 已结束 {killed} 个游戏进程");
    dispose(runtime);
}
fn walk(dir: &std::path::Path) -> usize {
    let mut count = 0usize;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        count += 1;
        if entry.path().is_dir() {
            count += walk(&entry.path());
        }
    }
    count
}
