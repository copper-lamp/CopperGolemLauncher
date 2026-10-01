package com.copperlamp.coppergolem

import android.app.Application
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
    override fun onCreate() {
        super.onCreate()
        SecretStoreBridge.initialize(this)
    }
}
