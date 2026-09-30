package com.copperlamp.coppergolem.game

import android.content.Context
import android.content.Intent
import android.content.res.AssetManager
import android.graphics.Color
import android.graphics.drawable.ColorDrawable
import android.os.Bundle
import com.mojang.minecraftpe.MainActivity
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean

/**
 * 真正的游戏宿主 Activity。
 *
 * 继承 Mojang `MainActivity`（编译自 `:minecraft` 模块），
 * 职责只有两件：
 *
 * 1. **存储重定向** —— `getFilesDir` / `getDataDir` / `getCacheDir` /
 *    `getDatabasePath` / `getExternalFilesDir` 全部指向当前实例目录，
 *    让每个导入实例拥有独立且互不干扰的存档 worlds、resource_packs 与数据库。
 * 2. **资源重定向** —— `getAssets` 返回挂载了用户 APK 的 `AssetManager`。
 *
 * 其余生命周期行为（`super.onCreate` 触发 native `MainActivity_create`、
 * `onPause` 的 `nativeSuspend`、`onDestroy` 的 `nativeShutdown`）全部交给父类。
 *
 * 注意：`libminecraftpe.so` 一旦 `System.load` 就无法卸载，因此同一进程内
 * **不允许**先后启动两个不同实例。第二次启动必须先结束当前游戏 Activity，
 * 由 [reportExit] 清空会话；Rust 侧串行化启动请求来保证这一点。
 */
class CopperGameActivity : MainActivity() {
    private val exitReported = AtomicBoolean(false)
    private var runtimeStarted = false
    private var instanceName: String = ""

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

    override fun onDestroy() {
        // isChangingConfigurations：旋转/分屏导致重建，不算玩家退出。
        // 正常退出时 isFinishing 为 true，这才是要上报的信号。
        val normalExit = runtimeStarted && isFinishing && !isChangingConfigurations
        if (normalExit) {
            reportExitOnce(CopperGameExitRecord.Reason.NORMAL)
        } else if (runtimeStarted) {
            trace().warning("Activity 销毁但非正常退出: finishing=$isFinishing changing=${isChangingConfigurations}")
        }
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
