package com.copperlamp.coppergolem.game

import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.res.AssetManager
import android.content.res.Resources
import android.os.Build
import android.os.SystemClock
import android.util.Log
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.util.zip.ZipFile

/**
 * 导入实例的运行时资源与原生库管理者。
 *
 * 职责边界：
 * - 从 `base.apk.levi` 与 `splits/*.apk.levi` 中把 `lib/<abi>/*.so` 解压到
 *   启动器缓存目录，做镜像重写、权限收紧，再交给 [System.load]；
 * - 通过反射 `AssetManager#addAssetPath` 把用户 APK 挂进资源表，让
 *   Mojang 运行时能读到 `assets/` 下的纹理、着色器与语言包；
 * - 把 Conscrypt 插到 JCA 首位（Minecraft 的 TLS 校验依赖）。
 *
 * 单例按实例缓存：同一进程内重复启动同一实例不重复解压。
 */
class CopperGamePackageManager private constructor(
    private val context: Context,
    private val instance: CopperGameInstance,
    private val listener: CopperGameRuntimePreparer.ProgressListener?
) {
    private val nativeLibDir: String
    private val applicationInfo: ApplicationInfo
    private val assetManager: AssetManager

    private val requiredLibs = arrayOf(
        "libc++_shared.so",
        "libfmod.so",
        "libMediaDecoders_Android.so",
        "libminecraftpe.so",
    )

    private val optionalLibs = arrayOf(
        "libHttpClient.Android.so",
    )

    private val extractableLibs = requiredLibs + optionalLibs

    /** 这些库随启动器打包，通过 `System.loadLibrary` 加载，不从用户 APK 解压。 */
    private val systemLoadedLibs = arrayOf(
        "libPlayFabMultiplayer.so",
        "libmaesdk.so",
        "libgxcore.so",
    )

    data class LibraryLoadResult(
        val name: String,
        val fileName: String,
        val source: String,
        val loaded: Boolean,
        val durationMs: Long,
        val detail: String? = null
    )

    init {
        report("GamePackageManager 初始化")
        val deviceAbi = CopperGameLayout.deviceAbi()
        val libDir = CopperGameLayout.runtimeLibDir(context, instance.name, deviceAbi)
        applicationInfo = buildApplicationInfo(libDir)
        nativeLibDir = applicationInfo.nativeLibraryDir
        report("原生库缓存目录: $nativeLibDir")

        extractLibraries()
        report("挂载 AssetManager")
        assetManager = createAssetManager()
        report("AssetManager 就绪")
        setupSecurityProvider()
        report("GamePackageManager 初始化完成")
    }

    /**
     * 构造给 Mojang 运行时使用的 `ApplicationInfo`。
     *
     * Minecraft 的 native 层按 `sourceDir` / `splitSourceDirs` 打开资源、
     * 按 `nativeLibraryDir` 与 `dataDir` 定位数据，因此必须整体指向用户
     * 实例目录，而不是启动器自己的包信息。
     */
    private fun buildApplicationInfo(libDir: File): ApplicationInfo {
        val info = ApplicationInfo()
        info.sourceDir = instance.baseApk.absolutePath
        info.publicSourceDir = info.sourceDir
        info.nativeLibraryDir = libDir.absolutePath
        info.packageName = instance.packageName
        info.dataDir = instance.gameDataDir().absolutePath
        if (instance.splitApks.isNotEmpty()) {
            info.splitSourceDirs = instance.splitApks.map { it.absolutePath }.toTypedArray()
            info.splitPublicSourceDirs = info.splitSourceDirs
        }
        return info
    }

    // ---------------------------------------------------------------- 原生库

    private fun extractLibraries() {
        val outputDir = File(nativeLibDir)
        if (!outputDir.exists() && !outputDir.mkdirs()) {
            throw IOException("无法创建原生库缓存目录: $outputDir")
        }

        val apkFiles = collectApkFiles()
        val manifestText = buildExtractionManifest(apkFiles)
        val manifestFile = File(outputDir, ".extraction_manifest")
        val markerMatches = try {
            manifestFile.isFile && manifestFile.readText() == manifestText
        } catch (_: Exception) {
            false
        }

        val cacheRequired = cacheRequiredLibs()
        var cacheHit = markerMatches
        if (cacheHit) {
            for (lib in cacheRequired) {
                val file = File(outputDir, lib)
                if (!file.isFile || file.length() == 0L) {
                    cacheHit = false
                    break
                }
            }
        }
        if (cacheHit) {
            report("原生库缓存命中: $outputDir")
            for (lib in extractableLibs) {
                runCatching { ensureReadOnly(File(outputDir, lib)) }
            }
            return
        }
        report("原生库缓存未命中，开始解压")

        val apkPaths = apkFiles.map { it.absolutePath }
        val deviceAbi = CopperGameLayout.deviceAbi()
        apkPaths.forEach { extractFromApk(it, outputDir, deviceAbi) }

        if (cacheRequired.any { !File(outputDir, it).isFile || File(outputDir, it).length() == 0L }) {
            report("首选 ABI $deviceAbi 缺少部分库，回退其它 ABI: ${CopperGameLayout.fallbackAbis(deviceAbi)}")
            CopperGameLayout.fallbackAbis(deviceAbi).forEach { abi ->
                apkPaths.forEach { extractFromApk(it, outputDir, abi) }
            }
        }

        val missing = cacheRequired.filter { !File(outputDir, it).isFile || File(outputDir, it).length() == 0L }
        if (missing.isNotEmpty()) {
            throw IllegalStateException("APK 缺少必需的原生库: ${missing.joinToString()}")
        }

        runCatching { manifestFile.writeText(manifestText) }
            .onFailure { Log.w(TAG, "写入解压清单失败: ${it.message}") }
        report("原生库解压完成")
    }

    private fun collectApkFiles(): List<File> {
        val paths = mutableListOf(instance.baseApk.absolutePath)
        paths += instance.splitApks.map { it.absolutePath }
        return paths.map(::File).filter { file ->
            val keep = file.isFile
            if (!keep) {
                Log.w(TAG, "APK 文件缺失: ${file.absolutePath}")
                report("APK 文件缺失: ${file.absolutePath}")
            }
            keep
        }.sortedBy { it.absolutePath }
    }

    /**
     * 缓存有效性指纹。
     *
     * 只看文件是否存在会被「换了版本但缓存目录同名」骗过去，所以把
     * 提取器版本、设备 ABI、是否需要 HttpClient 以及每个 APK 的路径/大小/mtime
     * 一起纳入指纹。
     */
    private fun buildExtractionManifest(apkFiles: List<File>): String = buildString {
        append("extractor=").append(EXTRACTOR_VERSION).append('\n')
        append("abi=").append(CopperGameLayout.deviceAbi()).append('\n')
        append("token=").append(CopperNativeImageGuard.TOKEN).append('\n')
        append("mode=coppergolem-isolated").append('\n')
        append("http=").append(shouldLoadHttpClient()).append('\n')
        apkFiles.forEach { file ->
            append("apk=")
                .append(file.absolutePath).append('|')
                .append(file.length()).append('|')
                .append(file.lastModified()).append('\n')
        }
    }

    private fun extractFromApk(apkPath: String, outputDir: File, abi: String) {
        val apkFile = File(apkPath)
        if (!apkFile.isFile) {
            Log.w(TAG, "APK 不存在: $apkPath")
            return
        }
        try {
            ZipFile(apkFile).use { zip ->
                val abiPrefix = "lib/$abi/"
                for (lib in extractableLibs) {
                    val entry = zip.getEntry("$abiPrefix$lib") ?: continue
                    val output = File(outputDir, lib)
                    copyStreamToReadOnlyFile(zip.getInputStream(entry), output)
                    if (!CopperNativeImageGuard.processRequired(output)) {
                        output.delete()
                        throw IOException("原生库镜像处理失败: ${output.name}")
                    }
                    report("已解压 ${output.name} ($abi)")
                }
            }
        } catch (error: Exception) {
            Log.w(TAG, "从 $apkPath 解压失败: ${error.message}")
        }
    }

    private fun copyStreamToReadOnlyFile(input: InputStream, output: File) {
        ensureParentDirectory(output)
        if (output.exists() && !output.delete()) {
            throw IOException("无法替换已有文件: ${output.absolutePath}")
        }
        val tempFile = File(output.absolutePath + ".tmp")
        if (tempFile.exists()) tempFile.delete()
        FileOutputStream(tempFile).use { out ->
            input.copyTo(out)
            out.fd.sync()
        }
        if (!tempFile.renameTo(output)) {
            tempFile.delete()
            throw IOException("无法重命名临时文件: ${output.absolutePath}")
        }
        ensureReadOnly(output)
    }

    private fun ensureParentDirectory(file: File) {
        val parent = file.parentFile ?: return
        if (!parent.exists() && !parent.mkdirs()) {
            throw IOException("无法创建目录: ${parent.absolutePath}")
        }
    }

    private fun ensureReadOnly(file: File) {
        if (!file.isFile) {
            throw IOException("不是普通文件: ${file.absolutePath}")
        }
        if (!file.setReadable(true, true) && !file.canRead()) {
            throw IOException("无法置为可读: ${file.absolutePath}")
        }
        if (!file.setReadOnly() && file.canWrite()) {
            throw IOException("无法置为只读: ${file.absolutePath}")
        }
    }

    private fun cacheRequiredLibs(): Array<String> =
        if (shouldLoadHttpClient()) requiredLibs + optionalLibs else requiredLibs

    // ------------------------------------------------------------------ 加载

    fun resolveLibraryPath(name: String): String? {
        val file = File(nativeLibDir, toLibraryFileName(name))
        return if (file.isFile && file.length() > 0L) file.absolutePath else null
    }

    @SuppressLint("UnsafeDynamicallyLoadedCode")
    fun loadLibraryDetailed(name: String): LibraryLoadResult {
        val fileName = toLibraryFileName(name)
        val normalized = normalizeLibraryName(name)
        val startedAt = SystemClock.elapsedRealtime()

        if (systemLoadedLibs.contains(fileName)) {
            val source = "启动器内置库"
            return try {
                if (normalized == "gxcore") {
                    if (!CopperNativeBridge.bootstrapGxCore()) {
                        return LibraryLoadResult(
                            normalized, fileName, source, false, elapsedSince(startedAt), "gxcore 引导失败"
                        )
                    }
                } else {
                    System.loadLibrary(normalized)
                }
                LibraryLoadResult(normalized, fileName, source, true, elapsedSince(startedAt))
            } catch (error: Throwable) {
                val detail = error.message ?: error.javaClass.simpleName
                Log.e(TAG, "加载 $fileName 失败: $detail")
                LibraryLoadResult(normalized, fileName, source, false, elapsedSince(startedAt), detail)
            }
        }

        val path = resolveLibraryPath(name)
        val libFile = path?.let(::File) ?: File(nativeLibDir, fileName)
        val source = "实例解压库缓存"
        if (!libFile.isFile || libFile.length() == 0L) {
            val detail = "$fileName 不在 $nativeLibDir"
            Log.w(TAG, detail)
            return LibraryLoadResult(normalized, fileName, source, false, elapsedSince(startedAt), detail)
        }
        return try {
            ensureReadOnly(libFile)
            System.load(libFile.absolutePath)
            LibraryLoadResult(normalized, fileName, source, true, elapsedSince(startedAt), libFile.absolutePath)
        } catch (error: Throwable) {
            val detail = error.message ?: error.javaClass.simpleName
            Log.e(TAG, "加载 $fileName 失败: $detail")
            LibraryLoadResult(normalized, fileName, source, false, elapsedSince(startedAt), detail)
        }
    }

    fun loadLibrary(name: String): Boolean = loadLibraryDetailed(name).loaded

    /**
     * 批量加载库并按进度回调。
     *
     * 返回逐个结果而不抛异常：调用方决定哪些是致命的（例如 `libminecraftpe.so`
     * 缺失必然起不来），哪些可以放过（可选的 HttpClient）。
     */
    fun loadAllLibraries(
        excludeLibs: Set<String> = emptySet(),
        progressStart: Int = 46,
        progressEnd: Int = 74
    ): List<LibraryLoadResult> {
        val allLibs = requiredLibs + systemLoadedLibs
        val loadable = allLibs.filterNot { lib ->
            excludeLibs.contains(normalizeLibraryName(lib)) || excludeLibs.contains(lib)
        }
        val total = loadable.size.coerceAtLeast(1)
        var loadIndex = 0
        val results = mutableListOf<LibraryLoadResult>()
        for (lib in allLibs) {
            val libName = normalizeLibraryName(lib)
            if (excludeLibs.contains(libName) || excludeLibs.contains(lib)) {
                listener?.onLog("跳过原生库: $lib")
                continue
            }
            loadIndex += 1
            val progress = progressStart + ((progressEnd - progressStart) * (loadIndex - 1) / total)
            listener?.onProgress(progress, "加载原生库", "$loadIndex/$total")
            val result = loadLibraryDetailed(libName)
            results += result
            listener?.onLog(
                if (result.loaded) "已加载原生库: ${result.fileName}"
                else "原生库加载失败: ${result.fileName} (${result.detail})"
            )
        }
        return results
    }

    // ---------------------------------------------------------------- 资源

    /**
     * 建立指向用户 APK 的 `AssetManager`。
     *
     * `addAssetPath` 是隐藏 API，只能反射调用；返回 0 表示该 APK 没有资源表，
     * 这种情况游戏会在启动后才崩，因此这里当作错误抛出让准备界面直接显示。
     */
    private fun createAssetManager(): AssetManager {
        val assets = AssetManager::class.java.newInstance()
        val addAssetPath = AssetManager::class.java.getMethod("addAssetPath", String::class.java)

        val paths = mutableListOf<String>()
        if (instance.baseApk.isFile) {
            paths += instance.baseApk.absolutePath
        } else {
            Log.w(TAG, "base APK 不存在: ${instance.baseApk.absolutePath}")
        }
        instance.splitApks.forEach { split ->
            if (split.isFile) paths += split.absolutePath else Log.w(TAG, "split APK 不存在: $split")
        }
        // 启动器自身资源放在最后，保证游戏资源优先级最高。
        paths += context.packageResourcePath

        for (path in paths) {
            try {
                val cookie = addAssetPath.invoke(assets, path) as Int
                if (cookie == 0) {
                    throw IllegalStateException("无法挂载游戏资源: $path")
                }
            } catch (error: Exception) {
                if (path == context.packageResourcePath) {
                    throw IllegalStateException("无法挂载启动器资源: ${error.message}", error)
                }
                throw IllegalStateException("无法挂载游戏资源 $path: ${error.message}", error)
            }
        }
        return assets
    }

    private fun setupSecurityProvider() {
        try {
            java.security.Security.insertProviderAt(org.conscrypt.Conscrypt.newProvider(), 1)
        } catch (error: Exception) {
            // 已有 Conscrypt 实例时重复插入会失败，不影响后续流程。
            Log.w(TAG, "Conscrypt 初始化跳过: ${error.message}")
        }
    }

    /** 从游戏资源表读取字符串（Minecraft 的 Firebase / 服务端配置都走这里）。 */
    fun getGameStringResource(name: String): String? = try {
        val resources = Resources(assetManager, context.resources.displayMetrics, context.resources.configuration)
        val id = resources.getIdentifier(name, "string", applicationInfo.packageName)
        if (id == 0) null else resources.getString(id).takeIf { it.isNotBlank() }
    } catch (error: Exception) {
        Log.w(TAG, "读取游戏字符串资源 $name 失败: ${error.message}")
        null
    }

    fun getAssets(): AssetManager = assetManager

    fun getApplicationInfo(): ApplicationInfo = applicationInfo

    private fun report(message: String) {
        if (listener != null) listener.onLog(message) else CopperGameTrace.ensure(null).mark(message)
    }

    companion object {
        private const val TAG = "CopperGamePackage"
        private const val EXTRACTOR_VERSION = 1

        @Volatile
        private var instance: CopperGamePackageManager? = null

        private var lastInstanceKey: String? = null

        @JvmStatic
        fun getInstance(
            context: Context,
            game: CopperGameInstance,
            listener: CopperGameRuntimePreparer.ProgressListener? = null
        ): CopperGamePackageManager = synchronized(this) {
            val key = "${game.name}|${game.baseApk.absolutePath}|${game.baseApk.length()}|${game.baseApk.lastModified()}"
            val current = instance
            if (current == null || key != lastInstanceKey) {
                instance = CopperGamePackageManager(context.applicationContext, game, listener)
                lastInstanceKey = key
            }
            instance!!
        }

        /** 进程重启或切换实例时清空缓存，避免复用上一次的解压结果。 */
        @JvmStatic
        fun reset() = synchronized(this) {
            instance = null
            lastInstanceKey = null
        }

        private fun toLibraryFileName(name: String): String =
            if (name.startsWith("lib") && name.endsWith(".so")) name
            else "lib${normalizeLibraryName(name)}.so"

        private fun normalizeLibraryName(name: String): String =
            name.removePrefix("lib").removeSuffix(".so")

        private fun elapsedSince(startedAt: Long): Long = SystemClock.elapsedRealtime() - startedAt

        fun is64BitDevice(): Boolean = Build.SUPPORTED_64_BIT_ABIS.isNotEmpty()
    }
}
