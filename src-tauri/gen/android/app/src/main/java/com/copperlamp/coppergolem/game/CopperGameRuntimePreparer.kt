package com.copperlamp.coppergolem.game

import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * 原生库加载顺序策略。
 *
 * Minecraft 各版本的原生库布局不同，加载顺序错一个就会在
 * `System.load` 或之后的符号解析阶段崩：
 *
 * - `>= 1.21.110`：引擎改由 `libmaesdk.so` 承载，`libminecraftpe.so` 之外
 *   还需要 `libPlayFabMultiplayer.so`；`libgxcore.so` 必须先引导。
 * - `>= 1.21.130`：`libHttpClient.Android.so` 从可选变为必需。
 * - 更早版本：经典四件套 `c++_shared` / `fmod` / `MediaDecoders_Android` /
 *   `minecraftpe`（外加 `gxcore`）。
 */
object CopperGameRuntimePreparer {

    interface ProgressListener {
        fun onProgress(progress: Int, status: String, detail: String? = null)
        fun onLog(message: String)
    }

    object NoopListener : ProgressListener {
        override fun onProgress(progress: Int, status: String, detail: String?) = Unit
        override fun onLog(message: String) = Unit
    }

    const val EXTRA_FILES_DIR = "com.copperlamp.coppergolem.extra.FILES_DIR"
    const val EXTRA_EXTERNAL_FILES_DIR = "com.copperlamp.coppergolem.extra.EXTERNAL_FILES_DIR"
    const val EXTRA_DATA_DIR = "com.copperlamp.coppergolem.extra.DATA_DIR"
    const val EXTRA_CACHE_DIR = "com.copperlamp.coppergolem.extra.CACHE_DIR"
    const val EXTRA_SOURCE_DIR = "com.copperlamp.coppergolem.extra.SOURCE_DIR"
    const val EXTRA_SPLIT_SOURCE_DIRS = "com.copperlamp.coppergolem.extra.SPLIT_SOURCE_DIRS"

    private const val TAG = "CopperGamePreparer"

    private const val MAESDK_VERSION = "1.21.110"
    private const val MAESDK_BETA_VERSION = "1.21.110.22"
    private const val HTTP_CLIENT_VERSION = "1.21.130"
    private const val HTTP_CLIENT_BETA_VERSION = "1.21.130.20"

    fun shouldLoadMaesdk(versionCode: String): Boolean =
        isVersionAtLeast(versionCode, betaAwareTarget(versionCode, MAESDK_VERSION, MAESDK_BETA_VERSION))

    fun shouldLoadHttpClient(versionCode: String): Boolean =
        isVersionAtLeast(versionCode, betaAwareTarget(versionCode, HTTP_CLIENT_VERSION, HTTP_CLIENT_BETA_VERSION))

    private fun betaAwareTarget(versionCode: String, stable: String, beta: String): String =
        if (versionCode.contains("beta")) beta else stable

    /** 逐段数字比较；无法解析时保守返回 false（走经典加载路径）。 */
    fun isVersionAtLeast(current: String, target: String): Boolean {
        if (current.isBlank()) return false
        val currentParts = current.replace(Regex("[^0-9.]"), "").split(".").mapNotNull { it.toIntOrNull() }
        val targetParts = target.split(".").mapNotNull { it.toIntOrNull() }
        for (index in 0 until maxOf(currentParts.size, targetParts.size)) {
            val a = currentParts.getOrNull(index) ?: 0
            val b = targetParts.getOrNull(index) ?: 0
            if (a != b) return a > b
        }
        return true
    }

    /**
     * 填充 Minecraft 运行时读取的所有 Intent extra。
     *
     * `MC_SRC` / `MC_SPLIT_SRC` 由 native 层直接消费，storage 系列由
     * [CopperGameActivity] 的 `getFilesDir` 等重写消费；两者缺一都会导致
     * 「能启动但读不到资源」或「存档写错目录」。
     */
    fun fillLaunchExtras(context: Context, launchIntent: Intent, game: CopperGameInstance, manager: CopperGamePackageManager) {
        val info = manager.getApplicationInfo()
        game.gameFilesDir().mkdirs()
        game.gameDataDir().mkdirs()
        game.gameCacheDir().mkdirs()

        launchIntent.putExtra(EXTRA_FILES_DIR, game.gameFilesDir().absolutePath)
        launchIntent.putExtra(EXTRA_EXTERNAL_FILES_DIR, game.gameFilesDir().absolutePath)
        launchIntent.putExtra(EXTRA_DATA_DIR, game.gameDataDir().absolutePath)
        launchIntent.putExtra(EXTRA_CACHE_DIR, game.gameCacheDir().absolutePath)
        launchIntent.putExtra(EXTRA_SOURCE_DIR, info.sourceDir)
        val splitDirs = info.splitSourceDirs
        if (splitDirs != null) {
            launchIntent.putStringArrayListExtra(EXTRA_SPLIT_SOURCE_DIRS, arrayListOf(*splitDirs))
        }
        launchIntent.putExtra(CopperGameInstance.EXTRA_INSTANCE, game.name)
        launchIntent.putExtra(CopperGameInstance.EXTRA_VERSION_CODE, game.versionCode)
        launchIntent.putExtra(CopperGameInstance.EXTRA_PACKAGE, game.packageName)
        launchIntent.putExtra("IS_INSTALLED", false)
        launchIntent.putExtra("VERSION_ISOLATION", true)
    }

    /**
     * Mojang `MainActivity.onResume` 用这四个 extra 初始化 Firebase。
     *
     * 默认值取自官方包资源表；实例资源里存在同名项时优先用实例的值，
     * 保证不同版本配置生效。
     */
    fun configureFirebaseExtras(launchIntent: Intent, manager: CopperGamePackageManager) {
        fun value(key: String, fallback: String): String =
            manager.getGameStringResource(key)?.takeIf { it.isNotBlank() } ?: fallback

        launchIntent.putExtra("MINECRAFT_FIREBASE_APP_ID", value("google_app_id", "1:486187589451:android:b2331110821fe2304bd2ce"))
        launchIntent.putExtra("MINECRAFT_FIREBASE_API_KEY", value("google_api_key", value("google_crash_reporting_api_key", "")))
        launchIntent.putExtra("MINECRAFT_FIREBASE_PROJECT_ID", value("project_id", "minecraft-bedrock-57580"))
        launchIntent.putExtra("MINECRAFT_FIREBASE_SENDER_ID", value("gcm_defaultSenderId", "486187589451"))
    }

    /**
     * 按版本选择加载顺序并真正 `System.load`。
     *
     * 只在 `libminecraftpe.so` 这类核心库失败时抛异常；可选库失败只记日志，
     * 避免因为某个次要能力缺失就完全起不来。
     */
    fun loadNativeLibraries(
        game: CopperGameInstance,
        manager: CopperGamePackageManager,
        listener: ProgressListener
    ) {
        val version = game.versionCode
        val maesdk = shouldLoadMaesdk(version)
        val httpClient = shouldLoadHttpClient(version)
        listener.onLog("加载策略: maesdk=$maesdk httpClient=$httpClient version=$version")

        if (maesdk) {
            val exclude = mutableSetOf<String>()
            if (httpClient) {
                exclude += "c++_shared"
                exclude += "HttpClient.Android"
            }
            if (!shouldLoadPlayFab(version)) {
                exclude += "PlayFabMultiplayer"
            }
            val failures = manager.loadAllLibraries(exclude, 46, 88).filterNot { it.loaded }
            if (failures.isNotEmpty()) {
                val details = failures.joinToString("\n") { "${it.fileName}: ${it.detail ?: "未知错误"}" }
                throw IllegalStateException("原生库加载失败:\n$details")
            }
            return
        }

        if (!httpClient) {
            requireLibrary(manager, "c++_shared", 46, listener)
        } else {
            requireLibrary(manager, "c++_shared", 44, listener)
            requireLibrary(manager, "HttpClient.Android", 48, listener)
        }
        requireLibrary(manager, "fmod", 54, listener)
        requireLibrary(manager, "MediaDecoders_Android", 60, listener)
        requireLibrary(manager, "minecraftpe", 68, listener)
        requireLibrary(manager, "gxcore", 74, listener)
    }

    private fun requireLibrary(
        manager: CopperGamePackageManager,
        name: String,
        progress: Int,
        listener: ProgressListener
    ) {
        val fileName = toLibraryFileName(name)
        listener.onProgress(progress, "加载原生库", fileName)
        val result = manager.loadLibraryDetailed(name)
        if (result.loaded) {
            listener.onLog("已加载原生库: ${result.fileName}")
        } else {
            listener.onLog("原生库加载失败: ${result.fileName}")
            throw IllegalStateException("无法加载 ${result.fileName}: ${result.detail ?: "未知错误"}")
        }
    }

    private fun shouldLoadPlayFab(versionCode: String): Boolean =
        isVersionAtLeast(versionCode, betaAwareTarget(versionCode, HTTP_CLIENT_VERSION, HTTP_CLIENT_BETA_VERSION))

    private fun toLibraryFileName(name: String): String =
        if (name.startsWith("lib") && name.endsWith(".so")) name else "lib${name.removePrefix("lib").removeSuffix(".so")}.so"

    fun logFailure(stage: String, error: Throwable) {
        Log.e(TAG, "$stage 失败: ${error.message}", error)
    }
}
