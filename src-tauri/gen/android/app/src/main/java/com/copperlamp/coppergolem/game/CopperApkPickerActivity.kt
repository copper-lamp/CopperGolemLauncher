package com.copperlamp.coppergolem.game

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.drawable.BitmapDrawable
import android.graphics.drawable.Drawable
import android.util.Base64
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.UUID
import java.util.concurrent.Executors

/**
 * SAF 导入暂存：把系统文件选择器返回的 `content://` 流写入导入事务目录。
 *
 * 为什么不直接用 `tauri-plugin-dialog`：
 * 该插件在 Android 上只回传 `content://` URI，既不落盘、也无法让 Rust
 * 侧读取——内核拿到的字符串不是文件系统路径。这里由宿主自己完成
 * 「选择 → 流式写入事务目录 → 写回结果文件」三步，Rust 侧只按
 * `android_apk_pick_result` 取结果文件中的绝对路径。
 *
 * 结果文件即文件信箱（与退出记录同构）：
 * `<filesDir>/data/apk_pick_result.json` = `{ requestId, path, displayName, error }`。
 * 键名与 Rust 侧 `platform::android::ApkPickResult` 的 camelCase 序列化一一对应，
 * 改键名必须两侧同步。
 *
 * 事务目录由内核在导入完成后整体落位或回滚；未完成的残留在下次进入本界面时
 * 按保留时长清理（见 [pruneTransactions]）。
 */
class CopperApkPickerActivity : Activity() {
    private var requestId: String = ""
    private var resolved = false

    /** 写入门闩：`compareAndSet(false, true)` 返回 true 表示已有一次在途，丢弃重复投递。 */
    private val writing = java.util.concurrent.atomic.AtomicBoolean(false)

    /** 单线程执行器：写入串行，且不占主线程。 */
    private val executor = Executors.newSingleThreadExecutor { runnable ->
        Thread(runnable, "copper-apk-import").apply { isDaemon = true }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestId = intent.getStringExtra(EXTRA_REQUEST_ID).orEmpty()
        if (!UUID_REGEX.matches(requestId)) {
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
            pruneTransactions()
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
        writeOffMainThread(uri)
    }

    /**
     * 把所选文件写入独立事务目录，完成后再收尾本界面。
     *
     * 写入必须离开主线程：大包在主线程处理会触发 ANR 并留下不完整文件。
     * 单线程执行器保证同一时刻只处理一个选择请求。
     */
    private fun writeOffMainThread(uri: Uri) {
        if (!writing.compareAndSet(false, true)) return
        val displayName = queryDisplayName(uri)
        executor.execute {
            val outcome = runCatching { writeToTransaction(uri, displayName) }
            runOnUiThread {
                outcome
                    .onSuccess { target -> publishResult(target.absolutePath, displayName) }
                    .onFailure { error ->
                        Log.e(TAG, "写入所选文件失败: ${error.message}", error)
                        publishError("写入所选文件失败: ${error.message ?: error.javaClass.simpleName}")
                    }
            }
        }
    }

    override fun onDestroy() {
        // `shutdown()` 让当前事务写入自然收尾，避免留下不完整文件。
        executor.shutdown()
        super.onDestroy()
    }

    /** 从内核登记记录解析并验证本次事务目标。 */
    private fun transactionRoot(): File? {
        val requestFile = File(File(filesDir, "data/apk_import_requests"), "$requestId.json")
        val record = runCatching { JSONObject(requestFile.readText()) }.getOrNull() ?: return null
        if (record.optString("requestId") != requestId) return null
        val path = record.optString("transactionPath")
        if (path.isBlank()) return null
        val transaction = File(path).canonicalFile
        val root = transaction.parentFile?.canonicalFile ?: return null
        val versionsRoot = root.parentFile ?: return null
        if (transaction.name != requestId || root.name != ".import") return null
        val defaultVersions = File(File(filesDir, "data"), "versions").canonicalFile
        val privateRoot = filesDir.canonicalFile
        if (versionsRoot != defaultVersions && !versionsRoot.toPath().startsWith(privateRoot.toPath())) return null
        return transaction
    }

    /**
     * 清理过期且符合请求 ID 格式的事务目录，避免进程异常退出后留下大文件。
     */
    private fun pruneTransactions() {
        val root = File(filesDir, "data/versions/.import")
        val directories = root.listFiles() ?: return
        val deadline = System.currentTimeMillis() - TRANSACTION_RETENTION_MS
        for (directory in directories) {
            if (!directory.isDirectory || directory.lastModified() > deadline) continue
            if (!UUID_REGEX.matches(directory.name)) continue
            if (!directory.deleteRecursively()) Log.w(TAG, "清理过期导入事务失败: ${directory.name}")
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
     * 流式写入本次唯一事务目录，只允许从后台线程调用。
     */
    private fun writeToTransaction(uri: Uri, displayName: String): File {
        val transaction = transactionRoot() ?: throw IOException("导入事务目标无效")
        if (!transaction.isDirectory) throw IOException("导入事务目录不存在: $transaction")
        if (transaction.list()?.isNotEmpty() == true) throw IOException("导入事务目录非空")
        val extension = displayName.substringAfterLast('.', "apk").lowercase()
            .takeIf { it.length in 2..8 && it.all(Char::isLetterOrDigit) }
            ?: "apk"
        val target = File(transaction, "source.$extension")

        val input = contentResolver.openInputStream(uri)
            ?: throw IOException("无法读取所选文件（可能缺少授权）")
        try {
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
        } catch (error: Throwable) {
            transaction.deleteRecursively()
            throw error
        }
        return target
    }

    private fun publishResult(path: String, displayName: String) {
        val icon = runCatching { extractPackageIcon(path, displayName) }
            .onFailure { Log.w(TAG, "读取 APK 图标失败: ${it.message}") }
            .getOrNull()
        publish(
            JSONObject()
                .put("requestId", requestId)
                .put("path", path)
                .put("displayName", displayName)
                .put("versionName", extractPackageVersion(path, displayName) ?: JSONObject.NULL)
                .put("iconPng", icon ?: JSONObject.NULL)
                .put("error", "")
        )
    }

    private fun extractPackageIcon(path: String, displayName: String): String? {
        val archive = if (displayName.endsWith(".apks", ignoreCase = true)) {
            val temp = File(cacheDir, "apk-icon-$requestId.apk")
            val zip = java.util.zip.ZipFile(path)
            try {
                val entry = zip.getEntry("base.apk") ?: return null
                zip.getInputStream(entry).use { input -> temp.outputStream().use(input::copyTo) }
            } finally { zip.close() }
            temp
        } else File(path)
        return try {
            @Suppress("DEPRECATION")
            val packageInfo = packageManager.getPackageArchiveInfo(archive.absolutePath, PackageManager.GET_ACTIVITIES)
                ?: return null
            val appInfo = packageInfo.applicationInfo ?: return null
            appInfo.sourceDir = archive.absolutePath
            appInfo.publicSourceDir = archive.absolutePath
            val drawable = appInfo.loadIcon(packageManager)
            val size = 256
            val bitmap = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap)
            drawable.setBounds(0, 0, size, size)
            drawable.draw(canvas)
            val output = java.io.ByteArrayOutputStream()
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, output)
            bitmap.recycle()
            "data:image/png;base64," + Base64.encodeToString(output.toByteArray(), Base64.NO_WRAP)
        } finally {
            if (archive.parentFile == cacheDir) archive.delete()
        }
    }

    private fun extractPackageVersion(path: String, displayName: String): String? {
        val archive = if (displayName.endsWith(".apks", ignoreCase = true)) {
            val temp = File(cacheDir, "apk-version-$requestId.apk")
            val zip = java.util.zip.ZipFile(path)
            try {
                val entry = zip.getEntry("base.apk") ?: return null
                zip.getInputStream(entry).use { input -> temp.outputStream().use(input::copyTo) }
            } finally { zip.close() }
            temp
        } else File(path)
        return try {
            @Suppress("DEPRECATION")
            packageManager.getPackageArchiveInfo(archive.absolutePath, 0)?.versionName
        } finally {
            if (archive.parentFile == cacheDir) archive.delete()
        }
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

        /** 未完成事务的最长保留时长（6 小时）。 */
        private const val TRANSACTION_RETENTION_MS = 6L * 60 * 60 * 1000
        private val UUID_REGEX = Regex("[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
        const val EXTRA_REQUEST_ID = "com.copperlamp.coppergolem.extra.PICK_REQUEST_ID"
        const val RESULT_FILE = "apk_pick_result.json"
        const val CANCELLED = "用户取消了选择"
        const val MIME_APK = "application/vnd.android.package-archive"
        const val MIME_APKS = "application/vnd.android.package-archive-multiple"
    }
}
