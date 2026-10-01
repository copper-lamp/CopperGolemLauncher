package com.copperlamp.coppergolem.game

import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.os.Build
import org.json.JSONObject
import java.io.File

/**
 * 一个已导入的 MCBE 实例。所有启动链路（准备、加载、退出）都只通过
 * [name] 与 Rust 侧 `<filesDir>/data/versions/<name>` 约定定位磁盘文件，
 * 绝不接受来自 Intent 的绝对路径，避免路径逃逸。
 */
data class CopperGameInstance(
    val name: String,
    /**
     * Minecraft 版本号，例如 `1.21.130.20`；决定原生库加载顺序。
     *
     * **唯一权威来源是实例目录下的 `version.json`**（由 Rust 在导入/安装时
     * 从 APK 的二进制 AndroidManifest 解码后写入）。Intent 里同名参数只当
     * 诊断提示：深链是 `exported="true"` 的，任何应用都能构造一个假的版本号，
     * 按它选原生库加载顺序等于把启动策略交给外部应用。
     */
    val versionCode: String,
    val packageName: String,
    val root: File,
    val baseApk: File,
    val splitApks: List<File>
) {
    fun gameFilesDir(): File = CopperGameLayout.gameFilesDir(root)

    fun gameDataDir(): File = CopperGameLayout.gameDataDir(root)

    fun gameCacheDir(): File = CopperGameLayout.gameCacheDir(root)

    companion object {
        const val EXTRA_INSTANCE = "com.copperlamp.coppergolem.extra.INSTANCE"

        /**
         * 调用方给的版本号**提示**（深链 query 中的 `version_name`）。
         *
         * 只用于和 `version.json` 交叉比对、发现调用链传错参数，不参与任何
         * 决策：安卓侧不再有「Intent extra 优先」这条路径。
         */
        const val EXTRA_VERSION_HINT = "com.copperlamp.coppergolem.extra.VERSION_HINT"

        const val EXTRA_PACKAGE = "com.copperlamp.coppergolem.extra.PACKAGE"

        /**
         * 从 Intent 解析实例并校验磁盘布局。
         *
         * 校验失败一律抛出，让上层把准备失败原样显示给用户，而不是带着
         * 半成品目录进入 Mojang 的 native 初始化。
         *
         * 实例目录按**精确名优先**定位（见 [CopperGameLayout.resolveInstanceDir]），
         * 版本号只认 `version.json`。
         */
        fun fromIntent(context: Context, intent: Intent): CopperGameInstance {
            val rawName = intent.getStringExtra(EXTRA_INSTANCE)
                ?: throw IllegalArgumentException("缺少实例名")
            require(rawName.isNotBlank()) { "实例名为空" }

            val root = CopperGameLayout.resolveInstanceDir(context, rawName)
            val name = root.name

            val baseApk = CopperGameLayout.baseApk(root)
            require(baseApk.isFile) { "实例缺少 ${CopperGameLayout.BASE_APK}" }
            require(baseApk.length() > 0L) { "${CopperGameLayout.BASE_APK} 为空" }

            val meta = readVersionMeta(root, name)
            val hint = intent.getStringExtra(EXTRA_VERSION_HINT).orEmpty().trim()
            if (hint.isNotEmpty() && hint != meta.versionName) {
                // 不当作错误：调用方（深链 / 事件）传的值本来就不可信，权威值在
                // version.json。留痕是为了在真机 logcat 里一眼看出是谁传错了。
                CopperGameTrace.ensure(intent).warning(
                    "调用方给的版本号 $hint 与 version.json 的 ${meta.versionName} 不一致，已按 version.json 执行"
                )
            }

            return CopperGameInstance(
                name = name,
                versionCode = meta.versionName,
                packageName = meta.packageName.ifBlank { CopperGameLayout.MC_PACKAGE },
                root = root,
                baseApk = baseApk,
                splitApks = CopperGameLayout.splitApks(root)
            )
        }

        /**
         * 读取 `<实例目录>/version.json` 里的安卓元数据。
         *
         * Rust 侧 `game_download_import_apk` 在导入后会做同样的自检，缺字段
         * 时导入直接失败；这里再校验一次是因为文件可能在导入之后被人为改动，
         * 而缺版本号会让原生库加载顺序静默退化。
         */
        fun readVersionMeta(root: File, name: String): VersionFileMeta {
            val file = File(root, CopperGameLayout.VERSION_JSON)
            require(file.isFile) { "实例 `$name` 缺少 ${CopperGameLayout.VERSION_JSON}" }
            val raw = try {
                file.readText()
            } catch (error: Exception) {
                throw IllegalStateException(
                    "实例 `$name` 的 ${CopperGameLayout.VERSION_JSON} 读取失败: ${error.message}"
                )
            }
            val json = try {
                JSONObject(raw)
            } catch (error: Exception) {
                throw IllegalStateException(
                    "实例 `$name` 的 ${CopperGameLayout.VERSION_JSON} 不是合法 JSON: ${error.message}"
                )
            }
            val android = json.optJSONObject("android")
                ?: throw IllegalStateException("实例 `$name` 缺少安卓元数据，无法作为安卓实例启动")

            val versionName = android.optString("versionName").trim()
            require(versionName.isNotEmpty()) {
                "实例 `$name` 缺少版本号，无法确定原生库加载顺序"
            }
            val packageName = android.optString("packageName").trim()
            return VersionFileMeta(
                name = json.optString("name").trim(),
                versionName = versionName,
                packageName = packageName,
                packageDir = android.optString("packageDir").trim(),
                libCacheDir = android.optString("libCacheDir").trim()
            )
        }

        /**
         * 用 `PackageManager` 校验 base APK 确实是 MCBE。
         *
         * 导入时 Rust 已做 ZIP 结构校验，这里补的是包名维度的最终确认：
         * 错误的 base APK 会让 `System.load("libminecraftpe.so")` 之后在
         * native 层以极难定位的方式崩溃。
         */
        fun verifyPackage(context: Context, instance: CopperGameInstance): PackageInfo {
            val flags = PackageManager.GET_META_DATA or
                PackageManager.MATCH_UNINSTALLED_PACKAGES
            val info = context.packageManager
                .getPackageArchiveInfo(instance.baseApk.absolutePath, flags)
                ?: throw IllegalStateException("无法解析 ${instance.baseApk.name} 的 AndroidManifest")
            require(info.packageName == CopperGameLayout.MC_PACKAGE) {
                "包名不是 ${CopperGameLayout.MC_PACKAGE}: ${info.packageName}"
            }
            info.applicationInfo?.let { appInfo ->
                appInfo.sourceDir = instance.baseApk.absolutePath
                appInfo.publicSourceDir = instance.baseApk.absolutePath
            }
            return info
        }

        /** 读取导入 APK 的 `versionName`（Minecraft 用的 `1.21.x.y` 形式）。 */
        fun readApkVersionName(context: Context, apk: File): String =
            try {
                context.packageManager
                    .getPackageArchiveInfo(apk.absolutePath, 0)
                    ?.versionName
                    .orEmpty()
            } catch (_: PackageManager.NameNotFoundException) {
                ""
            }

        fun longVersionCode(info: PackageInfo): Long =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) info.longVersionCode
            else info.versionCode.toLong()
    }
}

/** `version.json` 中安卓侧真正被消费的字段。 */
data class VersionFileMeta(
    val name: String,
    val versionName: String,
    val packageName: String,
    val packageDir: String,
    val libCacheDir: String
)
