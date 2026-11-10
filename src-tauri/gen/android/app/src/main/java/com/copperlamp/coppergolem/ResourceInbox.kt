package com.copperlamp.coppergolem

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import java.io.File
import java.util.UUID
import java.util.concurrent.Executors

/** Scoped URI grants are consumed on a bounded worker and never handed to Rust as paths. */
object ResourceInbox {
    const val REQUEST = 4317
    private val executor = Executors.newSingleThreadExecutor()
    private val accepted = setOf("mcpack", "mcaddon", "addon", "mcworld", "mctemplate", "zip", "cglm")
    fun pick(activity: Activity) {
        activity.startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "*/*"
            putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }, REQUEST)
    }
    fun receive(activity: Activity, intent: Intent?) {
        if (intent == null) return
        val uris = linkedSetOf<Uri>()
        if (intent.action == Intent.ACTION_VIEW || intent.action == null) intent.data?.let(uris::add)
        if (intent.action == Intent.ACTION_SEND) {
            @Suppress("DEPRECATION")
            (intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM))?.let(uris::add)
        }
        if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            @Suppress("DEPRECATION")
            intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)?.let(uris::addAll)
        }
        intent.clipData?.let { clip -> for (i in 0 until minOf(clip.itemCount, 32)) clip.getItemAt(i).uri?.let(uris::add) }
        val app = activity.applicationContext
        // Copy while the Activity's temporary read grant is live; no persisted grant is acquired.
        executor.execute {
            val inbox = File(app.cacheDir, "global-import").apply { mkdirs() }
            val deadline = System.currentTimeMillis() - 1_800_000L
            inbox.listFiles()?.filter { it.lastModified() < deadline }?.forEach { it.delete() }
            for (uri in uris.take(32)) {
                var partial: File? = null
                try {
                    require(uri.scheme == "content") { "Only content URIs are accepted" }
                    val name = app.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                        if (cursor.moveToFirst()) cursor.getString(0) else null
                    } ?: throw IllegalArgumentException("Missing resource name")
                    val extension = name.substringAfterLast('.', "").lowercase()
                    require(extension in accepted) { "Unsupported resource type" }
                    require((inbox.listFiles()?.size ?: 0) < 32) { "Import queue is full" }
                    val id = UUID.randomUUID().toString()
                    val target = File(inbox, "$id.$extension")
                    val staging = File(inbox, "$id.partial")
                    partial = staging
                    app.contentResolver.openInputStream(uri).use { input ->
                        requireNotNull(input) { "Unable to open selected file" }
                        staging.outputStream().use { output ->
                            val buffer = ByteArray(65536)
                            var total = 0L
                            while (true) {
                                val count = input.read(buffer)
                                if (count < 0) break
                                total += count
                                require(total <= 512L * 1024 * 1024) { "Resource exceeds 512 MiB" }
                                output.write(buffer, 0, count)
                            }
                            output.fd.sync()
                        }
                    }
                    check(staging.renameTo(target)) { "Unable to publish resource" }
                } catch (_: Exception) {
                    partial?.delete()
                    // Fixed code only: provider paths/messages can contain private information.
                    File(inbox, "${UUID.randomUUID()}.error").writeText("resource_copy_failed")
                }
            }
        }
    }
}
