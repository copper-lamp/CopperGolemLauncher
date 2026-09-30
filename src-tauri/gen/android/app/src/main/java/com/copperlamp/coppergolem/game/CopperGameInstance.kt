package com.copperlamp.coppergolem.game

import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.os.Build
import java.io.File

/**
 * 一个已导入的 MCBE 实例。所有启动链路（准备、加载、退出）都只通过
 * [name] 与 Rust 侧 `<filesDir>/data/versions/<name>` 约定定位磁盘文件，
 * 绝不接受来自 Intent 的绝对路径，避免路径逃逸。
 */
data class CopperGameInstance(
    val name: String,
    /** Minecraft 版本号，例如 `1.21.130.20`；用于选择原生库加载顺序。 */
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
        const val EXTRA_VERSION_CODE = "com.copperlamp.coppergolem.extra.VERSION_CODE"
        const val EXTRA_PACKAGE = "com.copperlamp.coppergolem.extra.PACKAGE"

        /**
         * 从 Intent 解析实例并校验磁盘布局。
         *
         * 校验失败一律抛出，让上层把准备失败原样显示给用户，而不是带着
         * 半成品目录进入 Mojang 的 native 初始化。
         */
        fun fromIntent(context: Context, intent: Intent): CopperGameInstance {
            val rawName = intent.getStringExtra(EXTRA_INSTANCE)
                ?: throw IllegalArgumentException("缺少实例名")
            require(rawName.isNotBlank()) { "实例名为空" }
            val name = CopperGameLayout.sanitizeInstance(rawName)
            require(name == rawName.trim()) { "实例名包含非法字符" }

            val root = CopperGameLayout.instanceDir(context, name)
            require(root.isDirectory) { "实例目录不存在: $name" }

            val baseApk = CopperGameLayout.baseApk(root)
            require(baseApk.isFile) { "实例缺少 ${CopperGameLayout.BASE_APK}" }
            require(baseApk.length() > 0L) { "${CopperGameLayout.BASE_APK} 为空" }

            return CopperGameInstance(
                name = name,
                versionCode = intent.getStringExtra(EXTRA_VERSION_CODE).orEmpty(),
                packageName = intent.getStringExtra(EXTRA_PACKAGE)
                    ?: CopperGameLayout.MC_PACKAGE,
                root = root,
                baseApk = baseApk,
                splitApks = CopperGameLayout.splitApks(root)
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
