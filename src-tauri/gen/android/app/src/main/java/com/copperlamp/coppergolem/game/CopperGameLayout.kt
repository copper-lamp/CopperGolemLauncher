package com.copperlamp.coppergolem.game

import android.content.Context
import java.io.File

/**
 * 安卓端游戏运行时的目录契约。
 *
 * 全部路径与 Rust 侧 `Paths::with_root(filesDir)` 保持一致：
 * `<filesDir>/data/versions/<instance>`，缓存落在 `<filesDir>/cache`。
 * 任何一侧改动都必须同步 [docs/安卓端适配.md]。
 */
object CopperGameLayout {
    /** Minecraft Bedrock 官方包名。 */
    const val MC_PACKAGE = "com.mojang.minecraftpe"

    /**
     * 导入的 base APK 文件名。
     *
     * 刻意不用 `base.apk`：应用私有目录里若存在可安装的 `.apk`，系统安装器、
     * 各类清理工具与部分安全扫描都会把它当成待安装包处理。用 `.apk.levi`
     * 后缀存储，既保留 ZIP 结构（AssetManager 与 ZipFile 只看内容不看后缀），
     * 又不会被系统识别为安装候选。
     */
    const val BASE_APK = "base.apk.levi"

    /** split APK 目录名。 */
    const val SPLITS_DIR = "splits"

    /** split APK 文件后缀，与 [BASE_APK] 同理。 */
    const val SPLIT_SUFFIX = ".apk.levi"

    /** 导入时由 Rust 写入的实例元数据。 */
    const val PACKAGE_JSON = "package.json"

    /** 退出记录文件名，供 Rust 侧读取（`<filesDir>/data/game_exit.json`）。 */
    const val EXIT_RECORD = "game_exit.json"

    /** 实例内游戏数据根目录名。 */
    const val GAME_DATA_DIR = "game"

    /** 把任意实例名规整为安全目录名，禁止路径分隔符与上跳。 */
    fun sanitizeInstance(name: String): String {
        val cleaned = name.trim().map { ch ->
            if (ch.isLetterOrDigit() || ch == '.' || ch == '_' || ch == '-') ch else '_'
        }.joinToString("")
        val trimmed = cleaned.trim('.', '_', '-')
        return if (trimmed.isEmpty()) "instance" else trimmed.take(96)
    }

    /** `<filesDir>/data/versions/<instance>`。 */
    fun instanceDir(context: Context, instance: String): File =
        File(File(context.filesDir, "data/versions"), sanitizeInstance(instance))

    fun baseApk(root: File): File = File(root, BASE_APK)

    fun splitsDir(root: File): File = File(root, SPLITS_DIR)

    /** 按名称排序返回全部 split APK，保证 AssetManager 挂载顺序稳定。 */
    fun splitApks(root: File): List<File> {
        val dir = splitsDir(root)
        val files = dir.listFiles() ?: return emptyList()
        return files.filter { it.isFile && it.name.endsWith(SPLIT_SUFFIX) }.sortedBy { it.name }
    }

    /** 原生库解压缓存：`<filesDir>/cache/runtime_libs/<instance>/<abiDir>`。 */
    fun runtimeLibDir(context: Context, instance: String, abi: String): File =
        File(File(File(context.filesDir, "cache/runtime_libs"), sanitizeInstance(instance)), abiToLibDir(abi))

    /** Minecraft 自己的 files 目录（存档、配置、resource_packs 等）。 */
    fun gameFilesDir(root: File): File = File(File(root, GAME_DATA_DIR), "files")

    /** Minecraft 自己的 data 目录（数据库）。 */
    fun gameDataDir(root: File): File = File(File(root, GAME_DATA_DIR), "data")

    /** Minecraft 自己的 cache 目录。 */
    fun gameCacheDir(root: File): File = File(File(root, GAME_DATA_DIR), "cache")

    /** 退出记录：Rust 通过 `android_game_take_exit` 读取并删除。 */
    fun exitRecordFile(context: Context): File =
        File(File(context.filesDir, "data"), EXIT_RECORD)

    /** Android ABI 名 → `System.loadLibrary` 使用的目录名（与 LeviLauncher 一致）。 */
    fun abiToLibDir(abi: String): String = when (abi) {
        "arm64-v8a" -> "arm64"
        "armeabi-v7a" -> "arm"
        else -> abi
    }

    /** 设备首选 64 位 ABI，缺失时回退到 32 位。 */
    fun deviceAbi(): String =
        android.os.Build.SUPPORTED_64_BIT_ABIS.firstOrNull { it == "arm64-v8a" || it == "x86_64" }
            ?: android.os.Build.SUPPORTED_32_BIT_ABIS.firstOrNull { it == "armeabi-v7a" || it == "x86" }
            ?: android.os.Build.SUPPORTED_ABIS.firstOrNull()
            ?: "armeabi-v7a"

    /** 设备上所有可用 ABI，按优先级排序，用于原生库缺失时回退。 */
    fun fallbackAbis(primary: String): List<String> =
        listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86").filter { it != primary }
}
