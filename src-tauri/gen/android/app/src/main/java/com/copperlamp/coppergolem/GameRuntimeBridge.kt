package com.copperlamp.coppergolem

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.content.res.AssetManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import androidx.games.activity.GameActivity
import java.io.File
import java.io.FileOutputStream
import java.util.zip.ZipFile

object GameRuntimeBridge {
  private const val TAG = "CopperGameRuntime"
  const val ACTION_PREPARE_GAME = "com.copperlamp.coppergolem.PREPARE_GAME"
  const val EXTRA_INSTANCE = "instance_name"

  fun prepare(activity: Activity, instanceName: String) {
    activity.startActivity(Intent(activity, GamePrepareActivity::class.java).putExtra(EXTRA_INSTANCE, instanceName))
  }

  data class PackageMetadata(val packageName: String, val versionName: String, val versionCode: Long, val sourcePath: String)

  fun readPackageMetadata(context: Context, apk: File): PackageMetadata {
    require(apk.isFile) { "APK 不存在: ${apk.path}" }
    val flags = PackageManager.GET_META_DATA or if (Build.VERSION.SDK_INT >= 21) PackageManager.MATCH_ALL else 0
    val info: PackageInfo = context.packageManager.getPackageArchiveInfo(apk.path, flags)
      ?: error("无法解析 APK AndroidManifest")
    val appInfo = info.applicationInfo ?: error("APK 缺少 ApplicationInfo")
    appInfo.sourceDir = apk.path
    appInfo.publicSourceDir = apk.path
    val versionCode = if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else info.versionCode.toLong()
    return PackageMetadata(info.packageName, info.versionName ?: "", versionCode, apk.path)
  }
}

class GameActivityHost : GameActivity() {
  private lateinit var runtimeRoot: File

  override fun onCreate(savedInstanceState: Bundle?) {
    runtimeRoot = File(filesDir, "data/versions/${intent.getStringExtra(GameRuntimeBridge.EXTRA_INSTANCE)}")
    if (!runtimeRoot.isDirectory) { finish(); return }
    super.onCreate(savedInstanceState)
  }

  override fun onDestroy() {
    val instance = intent.getStringExtra(GameRuntimeBridge.EXTRA_INSTANCE)
    if (!isChangingConfigurations && !instance.isNullOrBlank()) {
      sendBroadcast(Intent(this, GameExitReceiver::class.java)
        .setAction("com.copperlamp.coppergolem.GAME_EXITED")
        .putExtra(GameRuntimeBridge.EXTRA_INSTANCE, instance)
        .putExtra("reason", "activity_destroyed"))
    }
    super.onDestroy()
  }
}

class GameExitReceiver : android.content.BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    val instance = intent.getStringExtra(GameRuntimeBridge.EXTRA_INSTANCE) ?: return
    val reason = intent.getStringExtra("reason") ?: "destroyed"
    Log.i("CopperGameRuntime", "Game exited: $instance")
    GameRuntimeEvents.emitExit(context, instance, reason)
  }
}

object GameRuntimeEvents {
  fun emitExit(context: Context, instance: String, reason: String) {
    context.sendBroadcast(Intent(context, GameExitReceiver::class.java)
      .setAction("com.copperlamp.coppergolem.TAURI_GAME_EXITED")
      .putExtra(GameRuntimeBridge.EXTRA_INSTANCE, instance)
      .putExtra("reason", reason))
  }
}

class GamePrepareActivity : Activity() {
  companion object { private const val TAG = "CopperGamePrepare" }

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    val instance = intent.getStringExtra(GameRuntimeBridge.EXTRA_INSTANCE)
    if (instance.isNullOrBlank()) { fail("缺少实例名"); return }
    try {
      val root = File(filesDir, "data/versions/$instance")
      val base = File(root, "base.apk")
      val metadata = GameRuntimeBridge.readPackageMetadata(this, base)
      require(metadata.packageName == "com.mojang.minecraftpe") { "不是 Minecraft Bedrock APK" }
      val runtimeLibDir = File(filesDir, "runtime_libs/$instance/arm64")
      extractArm64Libraries(base, runtimeLibDir)
      loadLibraries(runtimeLibDir)
      val addAssetPath = AssetManager::class.java.getDeclaredMethod("addAssetPath", String::class.java)
      addAssetPath.isAccessible = true
      require((addAssetPath.invoke(resources.assets, base.path) as Int) != 0) { "无法挂载游戏资源" }
      File(root, "splits").listFiles()?.filter { it.isFile && it.extension == "apk" }?.sortedBy { it.name }?.forEach {
        addAssetPath.invoke(resources.assets, it.path)
      }
      startActivity(Intent(this, GameActivityHost::class.java).putExtra(GameRuntimeBridge.EXTRA_INSTANCE, instance))
      setResult(RESULT_OK)
      finish()
    } catch (t: Throwable) {
      Log.e(TAG, "runtime preparation failed", t)
      fail(t.message ?: "游戏运行时准备失败")
    }
  }

  private fun extractArm64Libraries(apk: File, target: File) {
    target.mkdirs()
    ZipFile(apk).use { zip ->
      zip.entries().asSequence().filter { it.name.startsWith("lib/arm64-v8a/") && it.name.endsWith(".so") }.forEach { entry ->
        val out = File(target, File(entry.name).name)
        zip.getInputStream(entry).use { input -> FileOutputStream(out).use { input.copyTo(it) } }
        out.setReadable(true, false)
      }
    }
  }

  private fun loadLibraries(dir: File) {
    require(File(dir, "libminecraftpe.so").isFile) { "APK 缺少 libminecraftpe.so" }
    val preferred = listOf("libc++_shared.so", "libfmod.so", "libMediaDecoders_Android.so", "libminecraftpe.so")
    val loaded = HashSet<String>()
    for (name in preferred + dir.listFiles().orEmpty().map { it.name }) {
      if (!loaded.add(name)) continue
      val file = File(dir, name)
      if (file.isFile) System.load(file.absolutePath)
    }
  }

  private fun fail(message: String) {
    setResult(RESULT_CANCELED, Intent().putExtra("error", message))
    finish()
  }
}
