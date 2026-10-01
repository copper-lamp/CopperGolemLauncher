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
     * AGDK 宿主契约声明的原生库名。
     *
     * `androidx.games.GameActivity` 在 `onCreate` 里按 Manifest 的
     * `android.app.lib_name` 反推 `lib<值>.so` 并 `System.loadLibrary`；
     * 不声明时按 applicationId 反推 `libcopperlamp_coppergolem.so`——
     * 这个文件不存在，结果是 `UnsatisfiedLinkError` 秒退。
     *
     * 取值必须是**启动器自己 jniLibs 里真实存在**的库：`gxcore` 随启动器
     * 打包（`app/src/main/jniLibs/<abi>/libgxcore.so`），准备阶段也已经通过
     * [CopperNativeBridge.ensureGxCoreLoaded] 加载过，宿主再加载一次只是命中
     * 已加载缓存。这里只做**自检**，不负责加载。
     *
     * 与 `app/src/main/AndroidManifest.xml` 里的 meta-data 是同一份契约，
     * 两处必须同时改。
     */
    const val DECLARED_NATIVE_LIBRARY = "gxcore"

    /** Manifest 中 AGDK 读取原生库名的 meta-data key。 */
    const val LIB_NAME_META_DATA = "android.app.lib_name"

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

    /** 版本元数据文件名（Rust 侧 `home::meta::META_FILE`）；安卓侧版本号的唯一权威。 */
    const val VERSION_JSON = "version.json"

    /** 退出记录文件名，供 Rust 侧读取（`<filesDir>/data/game_exit.json`）。 */
    const val EXIT_RECORD = "game_exit.json"

    /** 实例内游戏数据根目录名。 */
    const val GAME_DATA_DIR = "game"

    /** 实例名字符集以外的字符统一替换为该字符。 */
    private const val INSTANCE_NAME_FILLER = '_'

    /** 实例名长度上限（按字符计，与 Rust 侧一致）。 */
    const val INSTANCE_NAME_MAX_CHARS = 96

    /**
     * 把任意字符串规整为安全目录名。
     *
     * 与 Rust 侧 `meta::sanitize_instance_name` **逐条对应**（见
     * `docs/安卓端能力差距与优先级.md` P0-2）：去掉首尾空白 → 字符集外替换为
     * `_` → 连续 `_` 折叠 → 去掉首尾 `.` `_` `-` → 按字符截断 → 空则回落
     * `instance`。
     *
     * 关键点：这个名字既是 Rust 写下的目录名，也是这里定位实例的键。两侧规则
     * 只要有一处不同（此前 Kotlin 把空格换成 `_`、Rust 原样保留），就会变成
     * 「列表里看得到、点启动却说实例不存在」。
     */
    fun sanitizeInstance(name: String): String {
        val builder = StringBuilder(name.length)
        var lastWasFiller = false
        for (ch in name.trim()) {
            val allowed = (ch.isLetterOrDigit() && ch.code < 128) ||
                ch == '.' || ch == '_' || ch == '-'
            val mapped = if (allowed) ch else INSTANCE_NAME_FILLER
            if (mapped == INSTANCE_NAME_FILLER) {
                if (lastWasFiller) continue
                lastWasFiller = true
            } else {
                lastWasFiller = false
            }
            builder.append(mapped)
        }
        val trimmed = builder.toString().trim('.', '_', '-')
        if (trimmed.isEmpty()) return "instance"
        return trimmed.take(INSTANCE_NAME_MAX_CHARS)
    }

    /** 名字是否已是规整形态（规整后与自身一致）。 */
    fun isCanonicalInstanceName(name: String): Boolean =
        name.isNotBlank() && sanitizeInstance(name) == name

    /** `<filesDir>/data/versions`。 */
    fun versionsRoot(context: Context): File = File(File(context.filesDir, "data"), "versions")

    /**
     * 把请求里的实例名解析为磁盘上的实例目录。
     *
     * 优先**精确匹配同名目录**：实例目录名由 Rust 在导入 / 安装时确定，是唯一
     * 权威；即使历史遗留的名字带空格或非 ASCII，只要目录真的在那儿就按原名
     * 使用，绝不改名后再去找（改名即找不到）。
     *
     * 精确匹配失败时再尝试规整名，兼容两侧规则统一之前落下的目录。两次都不中
     * 直接抛错，让准备界面显示「实例目录不存在」，而不是让 native 层在半成品
     * 目录上崩。
     */
    fun resolveInstanceDir(context: Context, rawName: String): File {
        val requested = rawName.trim()
        require(requested.isNotEmpty()) { "实例名为空" }
        require(!requested.contains('/') && !requested.contains('\\')) { "实例名含路径分隔符" }
        require(requested != "." && requested != "..") { "实例名非法" }

        val root = versionsRoot(context)
        val exact = File(root, requested)
        if (exact.isDirectory) return exact

        val canonical = sanitizeInstance(requested)
        if (canonical != requested) {
            val fallback = File(root, canonical)
            if (fallback.isDirectory) return fallback
        }
        throw IllegalStateException("实例目录不存在: $requested")
    }

    /** `<filesDir>/data/versions/<instance>`（规整后拼接，写入路径用）。 */
    fun instanceDir(context: Context, instance: String): File =
        File(versionsRoot(context), sanitizeInstance(instance))

    fun baseApk(root: File): File = File(root, BASE_APK)

    fun splitsDir(root: File): File = File(root, SPLITS_DIR)

    /** 按名称排序返回全部 split APK，保证 AssetManager 挂载顺序稳定。 */
    fun splitApks(root: File): List<File> {
        val dir = splitsDir(root)
        val files = dir.listFiles() ?: return emptyList()
        return files.filter { it.isFile && it.name.endsWith(SPLIT_SUFFIX) }.sortedBy { it.name }
    }

    /**
     * 原生库解压缓存：`<filesDir>/cache/runtime_libs/<instance>/<abiDir>`。
     *
     * 目录名直接用**实例目录名**（[CopperGameInstance.name] 已由
     * [resolveInstanceDir] 定死），与 Rust 写入 `version.json` 的 `libCacheDir`
     * （`runtime_libs/<实例名>`）对齐。
     */
    fun runtimeLibDir(context: Context, instance: String, abi: String): File =
        File(File(File(context.filesDir, "cache/runtime_libs"), instance), abiToLibDir(abi))

    /** Minecraft 自己的 files 目录（存档、配置、resource_packs 等）。 */
    fun gameFilesDir(root: File): File = File(File(root, GAME_DATA_DIR), "files")

    /** Minecraft 自己的 data 目录（数据库）。 */
    fun gameDataDir(root: File): File = File(File(root, GAME_DATA_DIR), "data")

    /** Minecraft 自己的 cache 目录。 */
    fun gameCacheDir(root: File): File = File(File(root, GAME_DATA_DIR), "cache")

    /** 退出记录：Rust 通过 `android_game_take_exit` 读取并删除。 */
    fun exitRecordFile(context: Context): File =
        dataFile(context, EXIT_RECORD)

    /** SAF 选择结果：Rust 通过 `android_apk_pick_result` 读取并删除。 */
    fun dataResultFile(context: Context, name: String): File = dataFile(context, name)

    private fun dataFile(context: Context, name: String): File =
        File(File(context.filesDir, "data"), name)

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
