package com.copperlamp.coppergolem.game

import android.content.Context
import android.content.Intent
import android.content.res.AssetManager
import android.graphics.Color
import android.graphics.drawable.ColorDrawable
import android.os.Bundle
import android.view.MotionEvent
import android.view.ViewGroup
import android.widget.FrameLayout
import com.copperlamp.coppergolem.game.controls.ControlLayout
import com.copperlamp.coppergolem.game.controls.ControlOverlayView
import com.copperlamp.coppergolem.game.controls.InjectingInputSink
import com.mojang.minecraftpe.MainActivity
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean

/**
 * 真正的游戏宿主 Activity。
 *
 * 继承 Mojang `MainActivity`（编译自 `:minecraft` 模块），职责只有三件：
 *
 * 1. **存储重定向** —— `getFilesDir` / `getDataDir` / `getCacheDir` /
 *    `getDatabasePath` / `getExternalFilesDir` 全部指向当前实例目录，
 *    让每个导入实例拥有独立且互不干扰的存档 worlds、resource_packs 与数据库。
 * 2. **资源重定向** —— `getAssets` 返回挂载了用户 APK 的 `AssetManager`。
 * 3. **屏幕触控层** —— 在游戏视图之上叠一层 [ControlOverlayView]，并把控件操作
 *    翻译成合成输入交给游戏（见 [setupControls] 与 `controls/` 包）。
 *
 * 其余生命周期行为（`super.onCreate` 触发 native `MainActivity_create`、
 * `onPause` 的 `nativeSuspend`、`onDestroy` 的 `nativeShutdown`）全部交给父类。
 *
 * 父类的 native 桥接库由 Manifest 的 `android.app.lib_name` 声明（见
 * `AndroidManifest.xml` 与 [CopperGameLayout.DECLARED_NATIVE_LIBRARY]）：
 * AGDK 在 `onCreate` 里按它加载 `libgxcore.so`，而真正的游戏原生库
 * （`libminecraftpe.so` 等）已由准备阶段按版本顺序 `System.load` 完毕。
 *
 * 注意：`libminecraftpe.so` 一旦 `System.load` 就无法卸载，因此同一进程内
 * **不允许**先后启动两个不同实例。第二次启动必须先结束当前游戏 Activity，
 * 由 [reportExit] 清空会话；Rust 侧串行化启动请求来保证这一点。
 */
class CopperGameActivity : MainActivity() {
    private val exitReported = AtomicBoolean(false)
    private var runtimeStarted = false
    private var instanceName: String = ""

    /** 触控层（为空表示本次启动没挂上；游戏仍可玩，只是没有屏幕控件）。 */
    private var controls: ControlOverlayView? = null
    private var inputSink: InjectingInputSink? = null


    override fun onCreate(savedInstanceState: Bundle?) {
        trace().mark("游戏 Activity onCreate 进入")
        window.setBackgroundDrawable(ColorDrawable(Color.BLACK))

        if (savedInstanceState != null) {
            // 进程被系统回收后重建：native 运行时状态已丢失，直接收尾回到启动器。
            trace().warning("检测到实例重建，放弃恢复并返回启动器")
            reportExitOnce(CopperGameExitRecord.Reason.DESTROYED)
            super.onCreate(null)
            finish()
            return
        }

        val game = resolveRuntime()
        if (game == null) {
            trace().error("运行时未就绪，退出")
            reportExitOnce(CopperGameExitRecord.Reason.LAUNCH_FAILED)
            super.onCreate(null)
            finish()
            return
        }
        instanceName = game.name

        try {
            val manager = CopperGameSession.manager()
                ?: throw IllegalStateException("运行时资源管理器缺失")
            CopperGameRuntimePreparer.configureFirebaseExtras(intent, manager)
            applyStorageDirs()
            trace().mark("存储与 Firebase 配置完成，开始父类初始化")
            runtimeStarted = true
            super.onCreate(savedInstanceState)
            trace().milestone("父类 onCreate 完成，游戏运行时已启动")
            // 触控层必须在父类建好游戏视图之后再挂，否则拿不到它作为注入目标。
            setupControls(game)
        } catch (error: Throwable) {
            CopperGameRuntimePreparer.logFailure("游戏 Activity 初始化", error)
            trace().error("父类 onCreate 失败", error.message ?: error.javaClass.simpleName)
            reportExitOnce(CopperGameExitRecord.Reason.LAUNCH_FAILED)
            finish()
        }
    }

    /**
     * 取出准备阶段准备好的实例。
     *
     * 正常路径下 [CopperGameSession] 已由准备界面填充。缺失时退化为直接从
     * Intent 重建（此时原生库必须已经加载完毕），再由调用方校验。
     */
    private fun resolveRuntime(): CopperGameInstance? {
        CopperGameSession.instance()?.let { return it }
        val trace = trace()
        return try {
            val game = CopperGameInstance.fromIntent(applicationContext, intent)
            // 原生库必须已由准备阶段加载完成；此处只重建 Java 侧资源与存储映射。
            val manager = CopperGamePackageManager.getInstance(applicationContext, game, null)
            CopperGameSession.set(game, manager)
            trace.warning("会话缓存缺失，已从 Intent 重建运行时")
            game
        } catch (error: Throwable) {
            trace.error("重建运行时失败", error.message ?: error.javaClass.simpleName)
            null
        }
    }

    private fun applyStorageDirs() {
        game().gameFilesDir().mkdirs()
        game().gameDataDir().mkdirs()
        game().gameCacheDir().mkdirs()
    }

    private fun game(): CopperGameInstance =
        CopperGameSession.instance() ?: throw IllegalStateException("游戏实例会话已失效")

    private fun trace(): CopperGameTrace = CopperGameTrace.ensure(intent)

    private fun resolveFromExtra(key: String, fallback: File?): File {
        val path = intent?.getStringExtra(key)
        val dir = if (!path.isNullOrBlank()) File(path) else fallback ?: super.getFilesDir()
        if (!dir.exists()) dir.mkdirs()
        return dir
    }

    // -------------------------------------------------------------- 屏幕触控层

    /**
     * 挂载触控层。
     *
     * 布局读自实例目录的 `controls.json`（由 Rust 原子写入）；文件缺失或损坏时
     * [ControlLayout.load] 会退回默认布局并给出原因，因此这里**不会**因为没有
     * 布局而让游戏没法操作。
     *
     * 挂载失败（拿不到内容视图等）只记日志不抛：触控层是增强，游戏本体不该被它
     * 拖垮——玩家至少还能用真实触摸屏操作（未被控件覆盖的区域仍然直通游戏）。
     */
    private fun setupControls(game: CopperGameInstance) {
        try {
            val content = findViewById<ViewGroup>(android.R.id.content) ?: run {
                trace().warning("找不到 android.R.id.content，跳过触控层挂载")
                return
            }
            val loaded = ControlLayout.load(game.root)
            if (loaded.reason != null) {
                trace().warning("触控布局已兜底: ${loaded.reason}")
            }

            val sink = InjectingInputSink(this, gameSurfaceView())
            val overlay = ControlOverlayView(this, sink).apply {
                setLayout(loaded.layout)
            }
            // 作为内容视图的最后一个子 View = 最上层；只占自己控件的区域，
            // 未被控件覆盖的地方靠 onTouchEvent 返回 false 让事件透到游戏。
            content.addView(
                overlay,
                FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.MATCH_PARENT
                )
            )

            inputSink = sink
            controls = overlay
            trace().milestone("触控层已挂载：${loaded.layout.controls.size} 个控件")
        } catch (error: Throwable) {
            trace().error("触控层挂载失败（游戏仍可继续）", error.message ?: error.javaClass.simpleName)
        }
    }

    /**
     * 取 AGDK 的游戏 SurfaceView 作为触摸注入目标。
     *
     * 它是 `GameActivity` 的 `protected` 字段，子类可直接读；这里用反射是为了在
     * AGDK 改字段名时**优雅退化**（退回窗口 DecorView）而不是编译不过或崩溃。
     */
    private fun gameSurfaceView(): android.view.View? = try {
        val field = com.google.androidgamesdk.GameActivity::class.java
            .getDeclaredField("mSurfaceView")
            .apply { isAccessible = true }
        field.get(this) as? android.view.View
    } catch (error: Throwable) {
        trace().mark("未能取得游戏 SurfaceView，触摸注入退回 DecorView: ${error.javaClass.simpleName}")
        null
    }

    /**
     * 触摸分发：先给触控层，未被吃掉才交给游戏。
     *
     * 两层保护缺一不可：
     * 1. 触控层返回 `false`（未命中控件）时**不拦截**，真实触摸照常进游戏；
     * 2. 触控层为空时直接走父类，行为与未引入触控层前完全一致。
     *
     * 回环防护在 [InjectingInputSink.isInjectingTouch]（覆盖层自己放行注入中的事件），
     * 这里不再重复判断——同一件事只有一处真相，避免两处判断不一致。
     */
    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        val overlay = controls ?: return super.dispatchTouchEvent(event)
        if (overlay.onTouchEvent(event)) {
            return true
        }
        return super.dispatchTouchEvent(event)
    }

    // ------------------------------------------------------------ Mojang 契约

    override fun getAssets(): AssetManager {
        val manager = CopperGameSession.manager()
        return if (manager != null) manager.getAssets() else super.getAssets()
    }

    override fun getFilesDir(): File = resolveFromExtra(CopperGameRuntimePreparer.EXTRA_FILES_DIR, super.getFilesDir())

    override fun getDataDir(): File = resolveFromExtra(CopperGameRuntimePreparer.EXTRA_DATA_DIR, super.getDataDir())

    override fun getCacheDir(): File = resolveFromExtra(CopperGameRuntimePreparer.EXTRA_CACHE_DIR, super.getCacheDir())

    override fun getExternalFilesDir(type: String?): File? {
        val base = resolveFromExtra(
            CopperGameRuntimePreparer.EXTRA_EXTERNAL_FILES_DIR,
            super.getExternalFilesDir(null)
        )
        return if (type.isNullOrEmpty()) base else File(base, type).also { it.mkdirs() }
    }

    override fun getDatabasePath(name: String): File {
        val dir = File(getDataDir(), "databases")
        if (!dir.exists()) dir.mkdirs()
        return File(dir, name)
    }

    override fun getInternalStoragePath(): String = getFilesDir().absolutePath

    override fun getExternalStoragePath(): String = (getExternalFilesDir(null) ?: getFilesDir()).absolutePath

    override fun onNewIntent(intent: Intent) {
        setIntent(intent)
        super.onNewIntent(intent)
    }

    override fun onResume() {
        super.onResume()
        trace().mark("游戏 Activity onResume")
    }

    /**
     * 切到后台时释放所有按下的键。
     *
     * 不做这一步，玩家「按住 W 时被来电打断」会让 W 在游戏里一直按着（合成输入的
     * up 事件永远不会来），回到游戏后角色自己往前走。
     */
    override fun onPause() {
        controls?.releaseAll()
        inputSink?.releaseAll()
        super.onPause()
    }

    override fun onDestroy() {
        // isChangingConfigurations：旋转/分屏导致重建，不算玩家退出。
        // 正常退出时 isFinishing 为 true，这才是要上报的信号。
        val normalExit = runtimeStarted && isFinishing && !isChangingConfigurations
        if (normalExit) {
            reportExitOnce(CopperGameExitRecord.Reason.NORMAL)
        } else if (runtimeStarted) {
            trace().warning("Activity 销毁但非正常退出: finishing=$isFinishing changing=$isChangingConfigurations")
        }
        controls?.releaseAll()
        inputSink?.releaseAll()
        controls = null
        inputSink = null
        CopperGameSession.clear()
        trace().milestone("游戏 Activity onDestroy")
        try {
            super.onDestroy()
        } finally {
            // 原生库不可卸载：只有确认要回到启动器时才清空包管理器缓存，
            // 让下一次启动重新解压/校验。
            if (normalExit) {
                CopperGamePackageManager.reset()
            }
        }
    }

    private fun reportExitOnce(reason: String) {
        if (instanceName.isBlank() && CopperGameSession.instance() == null) return
        if (!exitReported.compareAndSet(false, true)) return
        val name = instanceName.ifBlank { CopperGameSession.instance()?.name.orEmpty() }
        if (name.isNotBlank()) {
            CopperGameExitRecord.report(applicationContext, name, reason)
        }
    }
}
