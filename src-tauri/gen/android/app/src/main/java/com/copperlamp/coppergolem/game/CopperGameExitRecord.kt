package com.copperlamp.coppergolem.game

import android.content.Context
import android.util.Log
import org.json.JSONObject
import java.io.File

/**
 * 游戏退出记录桥。
 *
 * 游戏 Activity 与启动器在同一进程，退出时 Mojang 的 native 库无法卸载，
 * 也没有可靠的 Java 回调能穿透到 Tauri 内核。因此这里采用**文件信箱**：
 *
 * 1. 游戏 Activity 销毁时把退出记录原子写入 `<filesDir>/data/game_exit.json`；
 * 2. Rust 侧命令 `android_game_take_exit` 读取后立即删除（take 语义，不重复消费）；
 * 3. 前端在窗口重新获得焦点、以及每次挂载时调用一次。
 *
 * 文件信箱不依赖 WebView 是否存活、也不依赖广播接收方是否已注册，
 * 这是当前唯一能保证「退出一定被内核观测到」的方式。
 */
object CopperGameExitRecord {
    private const val TAG = "CopperGameExit"

    /** 退出原因。 */
    object Reason {
        /** 玩家从游戏内正常退出 / 返回键关闭。 */
        const val NORMAL = "normal"

        /** Activity 被销毁但不属于正常退出（进程回收、配置变更）。 */
        const val DESTROYED = "destroyed"

        /** 启动失败，`MainActivity` 初始化抛异常。 */
        const val LAUNCH_FAILED = "launch_failed"
    }

    data class Record(val instance: String, val reason: String)

    @Volatile
    private var lastReported = 0L

    /**
     * 写入退出记录。
     *
     * 同一实例的重复 `onDestroy`（配置变更、任务切换）只记一次，
     * 用实例名 + 毫秒时间去重，避免把「切后台」误判成「游戏已退出」。
     */
    @JvmStatic
    @Synchronized
    fun report(context: Context, instance: String, reason: String) {
        if (instance.isBlank()) return
        val now = System.currentTimeMillis()
        if (now - lastReported < EXIT_DEBOUNCE_MS) {
            return
        }
        lastReported = now

        val file = CopperGameLayout.exitRecordFile(context)
        runCatching {
            file.parentFile?.mkdirs()
            val temp = File(file.absolutePath + ".tmp")
            temp.writeText(
                JSONObject()
                    .put("instance_name", instance)
                    .put("reason", reason)
                    .put("exited_at", now)
                    .toString()
            )
            if (!temp.renameTo(file)) {
                file.writeText(temp.readText())
                temp.delete()
            }
        }.onSuccess {
            Log.i(TAG, "已记录游戏退出: $instance ($reason)")
        }.onFailure {
            Log.e(TAG, "写入退出记录失败: ${it.message}")
        }
    }

    /** 允许在下一次启动前重置去重窗口。 */
    @JvmStatic
    fun resetDebounce() {
        lastReported = 0L
    }

    private const val EXIT_DEBOUNCE_MS = 1500L
}
