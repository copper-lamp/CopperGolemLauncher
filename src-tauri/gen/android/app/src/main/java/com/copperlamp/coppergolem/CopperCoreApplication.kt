package com.copperlamp.coppergolem

import android.app.Application
import android.util.Log
import com.copperlamp.coppergolem.secret.SecretStoreBridge

/**
 * 启动器进程级初始化。
 *
 * 存在的唯一理由是**初始化顺序**：凭证存储必须在任何账户读写之前拿到
 * `Context`，而账户读写可能发生在 Rust 内核启动阶段（早于任何 Activity）。
 * `Application.onCreate` 是安卓保证「早于一切组件」的唯一位置。
 *
 * 这里刻意不做别的初始化：路径、日志、数据库都归 Rust 内核，Kotlin 侧只保留
 * 「安卓机制」相关的绑定（与 docs/安卓适配-LeviLaunchroid调研与三功能方案.md
 * 2.3 的职责划分一致）。
 */
class CopperCoreApplication : Application() {
    private companion object {
        private const val TAG = "CopperCoreApplication"

        /** 与 Tauri 生成的 `Rust.<clinit>` 里 `System.loadLibrary` 的参数保持一致。 */
        private const val LIBRARY_NAME = "copper_core_lib"
    }

    override fun onCreate() {
        super.onCreate()

        // 顺序是硬约束，不能调换。
        //
        // SecretStoreBridge 的 `init` 块会调用 native 的 nativeRegisterBridge 交接桥类，
        // 而该符号由 libcopper_core_lib.so 提供。Application.onCreate 早于 WryActivity，
        // 若不在这里先把库加载起来，init 块就会在库加载之前调用 native 方法而
        // UnsatisfiedLinkError 失败——**且 `init` 块一生只执行一次**，之后即便 Rust
        // 自己把库加载完毕，桥类也再无机会交接，Keystore 将在整个进程内永久不可用
        // （真机实测现象见 docs/安卓端调试.md）。
        //
        // 这里显式「先加载、再初始化」；之后 Rust.<clinit> 的同名 loadLibrary 命中
        // 系统已加载缓存，是幂等的。
        try {
            System.loadLibrary(LIBRARY_NAME)
        } catch (error: UnsatisfiedLinkError) {
            Log.e(TAG, "加载原生库失败，凭证存储将不可用: ${error.message}")
        }

        SecretStoreBridge.initialize(this)
    }
}
