package com.copperlamp.coppergolem

import android.os.Bundle
import android.content.Intent
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  companion object {
    const val ACTION_PREPARE_GAME = "com.copperlamp.coppergolem.PREPARE_GAME"
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

  private fun handleGameIntent(intent: Intent?) {
    if (intent?.action == ACTION_PREPARE_GAME) {
      GameRuntimeBridge.prepare(this, intent.getStringExtra(GameRuntimeBridge.EXTRA_INSTANCE) ?: return)
    }
    if (intent?.data?.scheme == "coppergolem" && intent.data?.host == "game") {
      GameRuntimeBridge.prepare(this, intent.data?.getQueryParameter(GameRuntimeBridge.EXTRA_INSTANCE) ?: return)
    }
  }
}
