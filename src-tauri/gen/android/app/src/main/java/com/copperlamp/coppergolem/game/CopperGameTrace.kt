package com.copperlamp.coppergolem.game

import android.content.Intent
import android.os.SystemClock
import android.util.Log
import java.util.Locale
import java.util.UUID

/**
 * 启动链路追踪。
 *
 * 原生库加载与 Mojang `MainActivity` 初始化阶段没有任何可见 UI 输出，
 * 失败时只能靠 logcat 定位。这里给每次启动一个短会话号，把关键节点
 * 统一打到 `CopperGameTrace`，并同步冒泡到准备界面日志。
 */
class CopperGameTrace private constructor(
    private val sessionId: String,
    private val startedElapsedMs: Long
) {
    fun mark(stage: String, detail: String? = null) = record(stage, detail, Log.DEBUG)

    fun milestone(stage: String, detail: String? = null) = record(stage, detail, Log.INFO)

    fun warning(stage: String, detail: String? = null) = record(stage, detail, Log.WARN)

    fun error(stage: String, detail: String? = null) = record(stage, detail, Log.ERROR)

    private fun record(stage: String, detail: String?, level: Int) {
        val total = SystemClock.elapsedRealtime() - startedElapsedMs
        val suffix = if (detail.isNullOrEmpty()) "" else " - $detail"
        Log.println(level, TAG, String.format(Locale.US, "[%s] +%dms %s%s", sessionId, total, stage, suffix))
    }

    fun elapsedMs(): Long = SystemClock.elapsedRealtime() - startedElapsedMs

    companion object {
        private const val TAG = "CopperGameTrace"
        const val EXTRA_SESSION_ID = "com.copperlamp.coppergolem.extra.SESSION_ID"
        const val EXTRA_STARTED_MS = "com.copperlamp.coppergolem.extra.STARTED_MS"

        /** 优先复用上游写入的会话号，保证准备界面与游戏界面属于同一次启动。 */
        fun ensure(intent: Intent?): CopperGameTrace {
            val now = SystemClock.elapsedRealtime()
            if (intent != null && intent.hasExtra(EXTRA_SESSION_ID) && intent.hasExtra(EXTRA_STARTED_MS)) {
                val id = intent.getStringExtra(EXTRA_SESSION_ID)
                val start = intent.getLongExtra(EXTRA_STARTED_MS, now)
                if (!id.isNullOrBlank()) return CopperGameTrace(id, start)
            }
            val id = UUID.randomUUID().toString().substring(0, 8)
            intent?.putExtra(EXTRA_SESSION_ID, id)
            intent?.putExtra(EXTRA_STARTED_MS, now)
            return CopperGameTrace(id, now)
        }
    }
}
