package com.copperlamp.coppergolem.game

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.concurrent.Executors

/**
 * SAF 收件箱：把系统文件选择器返回的 `content://` 流落盘到应用私有目录。
 *
 * 为什么不直接用 `tauri-plugin-dialog`：
 * 该插件在 Android 上只回传 `content://` URI，既不落盘、也无法让 Rust
 * 侧读取——内核拿到的字符串不是文件系统路径。这里由宿主自己完成
 * 「选择 → 流式复制 → 写回结果文件」三步，Rust 侧只按
 * `android_apk_pick_result` 取结果文件中的绝对路径。
 *
 * 结果文件即文件信箱（与退出记录同构）：
 * `<filesDir>/data/apk_pick_result.json` = `{ requestId, path, displayName, error }`。
 * 键名与 Rust 侧 `platform::android::ApkPickResult` 的 camelCase 序列化一一对应，
 * 改键名必须两侧同步。
 *
 * 收件箱里的副本由内核在导入时读走，导入成功后本界面不负责删除（内核不暴露
 * 删除命令，多加一个命令只为清一个临时文件不划算）。残留副本在下次进入本
 * 界面时按保留时长清理（见 [pruneInbox]），避免反复失败把私有目录撑满。
 */
class CopperApkPickerActivity : Activity() {
    private var requestId: String = ""
    private var resolved = false

    /** 复制中的门闩：`compareAndSet(false, true)` 返回 true 表示已有一次在途，丢弃重复投递。 */
    private val copying = java.util.concurrent.atomic.AtomicBoolean(false)

    /** 单线程执行器：复制串行，且不占主线程。 */
    private val executor = Executors.newSingleThreadExecutor { runnable ->
        Thread(runnable, "copper-apk-inbox").apply { isDaemon = true }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestId = intent.getStringExtra(EXTRA_REQUEST_ID).orEmpty()
        if (requestId.isBlank()) {
            publishError("缺少请求 ID")
            return
        }
        val picker = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            // MCBE 官方包是单个 APK；.apks 打包在部分文件管理器里报 zip。
            type = "*/*"
            putExtra(
                Intent.EXTRA_MIME_TYPES,
                arrayOf(MIME_APK, MIME_APKS, "application/zip", "application/octet-stream")
            )
        }
        try {
            pruneInbox()
            startActivityForResult(picker, REQUEST_PICK)
        } catch (error: Throwable) {
            publishError("无法打开系统文件选择器: ${error.message ?: error.javaClass.simpleName}")
        }
    }

    @Deprecated("Deprecated in Java")
    @Suppress("DEPRECATION")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQUEST_PICK) return
        if (resultCode != RESULT_OK) {
            publishError(CANCELLED)
            return
        }
        val uri = data?.data
        if (uri == null) {
            publishError("未选择文件")
            return
        }
        copyOffMainThread(uri)
    }

    /**
     * 把所选文件复制到收件箱，完成后再收尾本界面。
     *
     * 复制必须离开主线程：MCBE 官方包 200MB 起步，在主线程上做这件事会直接
     * ANR——系统先弹「应用无响应」，用户关掉它，复制被腰斩，收件箱里只剩半个
     * 文件，前端轮询到超时也拿不到任何结果文件，只留下一句莫名其妙的「导入超时」。
     * 这里用单线程执行器串行处理：同一时刻只可能有一次导入在途。
     */
    private fun copyOffMainThread(uri: Uri) {
        if (copying.compareAndSet(false, true)) return
        val displayName = queryDisplayName(uri)
        executor.execute {
            val outcome = runCatching { copyToInbox(uri, displayName) }
            runOnUiThread {
                outcome
                    .onSuccess { target -> publishResult(target.absolutePath, displayName) }
                    .onFailure { error ->
                        Log.e(TAG, "复制所选文件失败: ${error.message}", error)
                        publishError("复制所选文件失败: ${error.message ?: error.javaClass.simpleName}")
                    }
            }
        }
    }

    override fun onDestroy() {
        // `shutdown()` 而不是 `shutdownNow()`：中断正在进行的复制只会换来收件箱里
        // 一个半截文件（比多留几秒更难排查），让复制自然收尾并照常写回结果。
        executor.shutdown()
        super.onDestroy()
    }

    /** 收件箱目录 `<filesDir>/cache/inbox`。 */
    private fun inboxDir(): File = File(filesDir, "cache/inbox")

    /**
     * 清掉过期残留。
     *
     * 导入失败、进程被杀、或用户选了包又在后续步骤反悔，都会留下 200MB 级的
     * 孤儿副本。这里按「修改时间早于 [INBOX_RETENTION_MS]」清理，既不会误删正在
     * 导入的副本（一次导入最多几分钟，远短于保留窗口），又能保证反复失败不会把
     * 应用私有目录撑满。
     */
    private fun pruneInbox() {
        val files = inboxDir().listFiles() ?: return
        val deadline = System.currentTimeMillis() - INBOX_RETENTION_MS
        for (file in files) {
            if (!file.isFile || file.lastModified() > deadline) continue
            if (!file.delete()) Log.w(TAG, "清理过期收件箱文件失败: ${file.name}")
        }
    }

    private fun queryDisplayName(uri: Uri): String = try {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) {
                val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (index >= 0) cursor.getString(index) else null
            } else {
                null
            }
        } ?: uri.lastPathSegment
    } catch (_: Throwable) {
        uri.lastPathSegment
    } ?: "import.apk"

    /**
     * 流式复制到 `cache/inbox/`。
     *
     * MCBE 包动辄 200MB 以上，必须按块复制而不是 `readBytes()`：
     * 一次性读入会直接 OOM，且大文件路径上系统临时目录也可能不可写。
     *
     * 只允许从 [copyOffMainThread] 的后台线程调用：在主线程上执行会 ANR。
     */
    private fun copyToInbox(uri: Uri, displayName: String): File {
        val inbox = inboxDir()
        if (!inbox.exists() && !inbox.mkdirs()) {
            throw IOException("无法创建收件箱目录: $inbox")
        }
        val extension = displayName.substringAfterLast('.', "apk").lowercase()
            .takeIf { it.length in 2..8 && it.all(Char::isLetterOrDigit) }
            ?: "apk"
        val target = File(inbox, "import-${System.nanoTime()}.$extension")

        val input = contentResolver.openInputStream(uri)
            ?: throw IOException("无法读取所选文件（可能缺少授权）")
        input.use { source ->
            FileOutputStream(target).use { output ->
                val buffer = ByteArray(1024 * 1024)
                var total = 0L
                while (true) {
                    val read = source.read(buffer)
                    if (read <= 0) break
                    output.write(buffer, 0, read)
                    total += read
                }
                output.fd.sync()
                if (total <= 0L) throw IOException("所选文件为空")
            }
        }
        return target
    }

    private fun publishResult(path: String, displayName: String) {
        publish(
            JSONObject()
                .put("requestId", requestId)
                .put("path", path)
                .put("displayName", displayName)
                .put("error", "")
        )
    }

    private fun publishError(message: String) {
        publish(
            JSONObject()
                .put("requestId", requestId)
                .put("path", "")
                .put("displayName", "")
                .put("error", message)
        )
    }

    private fun publish(payload: JSONObject) {
        if (resolved) return
        resolved = true
        runCatching {
            val file = CopperGameLayout.dataResultFile(this, RESULT_FILE)
            file.parentFile?.mkdirs()
            val temp = File(file.absolutePath + ".tmp")
            temp.writeText(payload.toString())
            if (!temp.renameTo(file)) {
                file.writeText(temp.readText())
                temp.delete()
            }
        }.onFailure { Log.e(TAG, "写回选择结果失败: ${it.message}") }
        finish()
        overridePendingTransition(0, 0)
    }

    companion object {
        private const val TAG = "CopperApkPicker"
        private const val REQUEST_PICK = 0x4150

        /** 收件箱残留保留时长（6 小时）：远大于一次导入耗时，远小于用户再次尝试的间隔。 */
        private const val INBOX_RETENTION_MS = 6L * 60 * 60 * 1000
        const val EXTRA_REQUEST_ID = "com.copperlamp.coppergolem.extra.PICK_REQUEST_ID"
        const val RESULT_FILE = "apk_pick_result.json"
        const val CANCELLED = "用户取消了选择"
        const val MIME_APK = "application/vnd.android.package-archive"
        const val MIME_APKS = "application/vnd.android.package-archive-multiple"
    }
}
