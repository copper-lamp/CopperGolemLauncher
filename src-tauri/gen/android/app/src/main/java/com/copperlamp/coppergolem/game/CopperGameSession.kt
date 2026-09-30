package com.copperlamp.coppergolem.game

import android.content.Intent

/**
 * 准备界面与游戏界面之间的进程内交接。
 *
 * 两者在同一进程，但生命周期不同：准备界面 `finish()` 后实例信息不能靠
 * Intent 重复传递（native 层会消费并改写部分 extra），因此显式持有。
 */
object CopperGameSession {
    @Volatile
    private var instance: CopperGameInstance? = null

    @Volatile
    private var manager: CopperGamePackageManager? = null

    fun set(instance: CopperGameInstance, manager: CopperGamePackageManager) {
        this.instance = instance
        this.manager = manager
    }

    fun instance(): CopperGameInstance? = instance

    fun manager(): CopperGamePackageManager? = manager

    fun clear() {
        instance = null
        manager = null
    }

    /** 游戏 Activity 退出时回填退出记录，由调用方决定实例名。 */
    fun reportExit(context: android.content.Context, reason: String) {
        instance?.let { CopperGameExitRecord.report(context, it.name, reason) }
        clear()
    }
}
