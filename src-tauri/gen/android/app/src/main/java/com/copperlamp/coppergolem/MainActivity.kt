package com.copperlamp.coppergolem

import android.content.Intent
import android.os.Bundle
import android.util.Log
import androidx.activity.enableEdgeToEdge
import com.copperlamp.coppergolem.game.CopperGameExitRecord
import com.copperlamp.coppergolem.game.CopperGameInstance
import com.copperlamp.coppergolem.game.CopperGamePrepareActivity
import com.copperlamp.coppergolem.game.CopperGameSession

/**
 * 启动器主界面（Tauri WebView 宿主）。
 *
 * 这里只做两件事：
 * 1. 把 Rust 内核通过事件/深链下发的启动请求转交给 [CopperGamePrepareActivity]；
 * 2. 在回到前台时消费游戏退出记录（见 [CopperGameExitRecord] 的文件信箱说明）。
 *
 * 游戏运行时的全部逻辑都在 `com.copperlamp.coppergolem.game` 包内，
 * 主界面不感知原生库与资源挂载细节。
 */
class MainActivity : TauriActivity() {
    companion object {
        /** Rust/前端直接唤起准备界面的显式 Action。 */
        const val ACTION_PREPARE_GAME = "com.copperlamp.coppergolem.PREPARE_GAME"

        private const val TAG = "CopperMainActivity"
        private const val DEEP_LINK_HOST = "game"
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        handleGameIntent(intent)
    }

    override fun onNewIntent(intent: Intent?) {
        super.onNewIntent(intent)
        handleGameIntent(intent)
    }

    /**
     * 解析启动请求。
     *
     * 两条入口都只携带**实例名**，实际路径由 [CopperGameLayout] 在应用私有
     * 目录内推导，从根本上杜绝外部传入任意路径。
     */
    private fun handleGameIntent(intent: Intent?) {
        if (intent == null) return
        val instance = when {
            intent.action == ACTION_PREPARE_GAME ->
                intent.getStringExtra(CopperGameInstance.EXTRA_INSTANCE)
            intent.data?.scheme == "coppergolem" && intent.data?.host == DEEP_LINK_HOST ->
                intent.data?.getQueryParameter("instance_name")
            else -> null
        } ?: return

        if (instance.isNullOrBlank()) {
            Log.w(TAG, "启动请求缺少实例名，已忽略")
            return
        }
        startActivity(
            Intent(this, CopperGamePrepareActivity::class.java)
                .putExtra(CopperGameInstance.EXTRA_INSTANCE, instance)
                .putExtra(
                    CopperGameInstance.EXTRA_VERSION_CODE,
                    intent.getStringExtra(CopperGameInstance.EXTRA_VERSION_CODE).orEmpty()
                )
        )
    }

    /**
     * 游戏退出后回到启动器。
     *
     * 原生库不可卸载，因此这里只做会话清理；真正决定「是否已退出」的是
     * [CopperGameExitRecord] 写下的退出记录，由 Rust 侧 `android_game_take_exit` 取走。
     */
    override fun onResume() {
        super.onResume()
        if (CopperGameSession.instance() == null) {
            // 回到启动器且没有活跃游戏实例：属于正常退出后的恢复路径。
            CopperGameExitRecord.resetDebounce()
        }
    }

    override fun onDestroy() {
        Log.d(TAG, "MainActivity onDestroy")
        super.onDestroy()
    }
}
