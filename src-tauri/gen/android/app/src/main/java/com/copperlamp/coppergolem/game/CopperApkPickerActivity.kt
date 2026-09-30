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
 */
class CopperApkPickerActivity : Activity() {
    private var requestId: String = ""
    private var resolved = false

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
        try {
            val displayName = queryDisplayName(uri)
            val target = copyToInbox(uri, displayName)
            publishResult(target.absolutePath, displayName)
        } catch (error: Throwable) {
            Log.e(TAG, "复制所选文件失败: ${error.message}", error)
            publishError("复制所选文件失败: ${error.message ?: error.javaClass.simpleName}")
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
     */
    private fun copyToInbox(uri: Uri, displayName: String): File {
        val inbox = File(filesDir, "cache/inbox")
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
        const val EXTRA_REQUEST_ID = "com.copperlamp.coppergolem.extra.PICK_REQUEST_ID"
        const val RESULT_FILE = "apk_pick_result.json"
        const val CANCELLED = "用户取消了选择"
        const val MIME_APK = "application/vnd.android.package-archive"
        const val MIME_APKS = "application/vnd.android.package-archive-multiple"
    }
}
