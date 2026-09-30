package com.copperlamp.coppergolem.game

import android.util.Log

/**
 * `libgxcore.so` 的 JNI 桥。
 *
 * `libgxcore.so` 随启动器一起打包（`app/src/main/jniLibs/arm64-v8a`），
 * 提供三件事：
 * 1. `bootstrapGxCore`：在加载 Minecraft 自身原生库之前完成引擎侧引导；
 * 2. `scanImage` / `rewriteImage`：处理从用户 APK 中解出的 `.so`。
 *
 * 这些 `external` 方法由 native 侧按名字查找，混淆会导致 UnsatisfiedLinkError，
 * 对应 keep 规则见 `app/proguard-rules.pro`。
 */
object CopperNativeBridge {
    private const val TAG = "CopperNativeBridge"

    @Volatile
    private var gxcoreLoaded = false

    /** 幂等地加载启动器自带的 gxcore。返回是否可用。 */
    @JvmStatic
    fun ensureGxCoreLoaded(): Boolean {
        if (gxcoreLoaded) return true
        return synchronized(this) {
            if (gxcoreLoaded) {
                true
            } else {
                try {
                    System.loadLibrary("gxcore")
                    gxcoreLoaded = true
                    true
                } catch (error: Throwable) {
                    Log.e(TAG, "System.loadLibrary(gxcore) failed: ${error.message}")
                    false
                }
            }
        }
    }

    /** 该 `.so` 是否需要重写。 */
    @JvmStatic
    fun scanImage(path: String): Boolean =
        ensureGxCoreLoaded() && nativeScanImage(path)

    /** 把 `input` 重写到 `output`，调用方负责原子替换与权限收紧。 */
    @JvmStatic
    fun rewriteImage(input: String, output: String): Boolean =
        ensureGxCoreLoaded() && nativeRewriteImage(input, output)

    /** Minecraft 1.21.110+ 需要先引导 gxcore，再加载 maesdk。 */
    @JvmStatic
    fun bootstrapGxCore(): Boolean {
        if (!ensureGxCoreLoaded()) return false
        return try {
            nativeBootstrapGxCore()
            true
        } catch (error: Throwable) {
            Log.e(TAG, "nativeBootstrapGxCore failed: ${error.message}")
            false
        }
    }

    @JvmStatic private external fun nativeScanImage(inputPath: String): Boolean

    @JvmStatic private external fun nativeRewriteImage(inputPath: String, outputPath: String): Boolean

    @JvmStatic private external fun nativeBootstrapGxCore()
}
