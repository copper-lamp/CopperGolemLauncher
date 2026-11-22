package com.copperlamp.coppergolem.game

import android.app.Activity
import android.content.Intent
import com.copperlamp.coppergolem.MainActivity
import java.lang.ref.WeakReference
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/** JNI entry point for Rust game launch requests; Activity ownership stays on the UI thread. */
object CopperGameHostBridge {
    private const val TAG = "CopperGameHostBridge"
    private var activity = WeakReference<MainActivity>(null)

    init {
        nativeRegisterBridge(CopperGameHostBridge::class.java)
    }

    @JvmStatic
    fun attach(host: MainActivity) {
        activity = WeakReference(host)
        nativeRegisterHost(host)
    }

    @JvmStatic
    fun detach(host: MainActivity) {
        if (activity.get() === host) {
            activity.clear()
            nativeClearHost(host)
        }
    }

    @JvmStatic
    fun prepare(instance: String) {
        val host = activity.get()
            ?: throw IllegalStateException("启动器 Activity 不可用")
        if (host.isFinishing || host.isDestroyed) {
            throw IllegalStateException("启动器 Activity 正在关闭")
        }
        val failure = AtomicReference<Throwable?>(null)
        val completed = CountDownLatch(1)
        val launch = {
            try {
                host.startActivity(
                    Intent(host, CopperGamePrepareActivity::class.java)
                        .putExtra(CopperGameInstance.EXTRA_INSTANCE, instance)
                )
            } catch (error: Throwable) {
                failure.set(error)
            } finally {
                completed.countDown()
            }
        }
        if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) launch()
        else host.runOnUiThread(launch)

        if (!completed.await(10, TimeUnit.SECONDS)) {
            throw IllegalStateException("等待 Android 启动界面超时")
        }
        failure.get()?.let { throw IllegalStateException("无法打开游戏准备界面", it) }
    }

    @JvmStatic private external fun nativeRegisterHost(host: Activity)
    @JvmStatic private external fun nativeClearHost(host: Activity)
    @JvmStatic private external fun nativeRegisterBridge(bridge: Class<*>)
}
