package com.copperlamp.coppergolem.game

import android.app.Activity
import android.content.Intent
import android.content.pm.ActivityInfo
import android.graphics.Color
import android.graphics.drawable.ColorDrawable
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.ScrollView
import android.widget.TextView
import java.text.SimpleDateFormat
import java.util.ArrayDeque
import java.util.Date
import java.util.Locale
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * 启动准备界面。
 *
 * 原生库解压 + 镜像重写 + 引导 gxcore 在 arm64 上通常要数秒到数十秒，
 * 必须有可见进度与可复制的错误日志：这些阶段完全没有游戏画面，
 * 失败时若只留一个黑屏，用户和开发者都无法定位。
 *
 * 流程：`onCreate` 先渲染界面并等首帧，再在后台线程执行
 * [CopperGameRuntimePreparer]，成功后进入 [CopperGameActivity] 并 finish 自己。
 */
class CopperGamePrepareActivity : Activity(), CopperGameRuntimePreparer.ProgressListener {
    private val mainHandler = Handler(Looper.getMainLooper())
    private val executor = Executors.newSingleThreadExecutor()
    private val timeFormat = SimpleDateFormat("HH:mm:ss", Locale.US)
    private val visibleLog = ArrayDeque<String>()
    private val preparingStarted = AtomicBoolean(false)
    private val returningToLauncher = AtomicBoolean(false)

    private lateinit var progressBar: ProgressBar
    private lateinit var statusView: TextView
    private lateinit var detailView: TextView
    private lateinit var logView: TextView
    private lateinit var logScroll: ScrollView
    private lateinit var returnButton: Button
    private val accentColor: Int by lazy { resolveAccentColor() }

    private var trace: CopperGameTrace? = null
    private var lastLogMessage: String? = null
    private var currentProgress = 0
    private var enteringGame = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        window.addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN)
        window.setBackgroundDrawable(ColorDrawable(backgroundColor()))
        hideSystemUi()

        trace = CopperGameTrace.ensure(intent)

        buildUi()
        CopperGamePackageManager.reset()
        CopperGameExitRecord.resetDebounce()
        trace?.milestone("准备界面就绪")

        appendLog("正在解析实例")
        startPreparingAfterFirstDraw()
    }

    // ------------------------------------------------------------------ 界面

    private fun backgroundColor(): Int =
        if (isDarkMode()) Color.rgb(16, 18, 20) else Color.rgb(250, 250, 250)

    private fun secondaryTextColor(): Int =
        if (isDarkMode()) Color.rgb(168, 174, 180) else Color.rgb(96, 104, 112)

    private fun primaryTextColor(): Int =
        if (isDarkMode()) Color.rgb(236, 238, 240) else Color.rgb(28, 32, 36)

    private fun resolveAccentColor(): Int {
        val typed = TypedValue()
        return if (theme.resolveAttribute(android.R.attr.colorAccent, typed, true)) {
            typed.data
        } else {
            Color.rgb(198, 124, 46)
        }
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density + 0.5f).toInt()

    private fun buildUi() {
        val root = FrameLayout(this).apply { setBackgroundColor(backgroundColor()) }

        val content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            setPadding(dp(32), dp(28), dp(32), dp(24))
        }
        root.addView(
            content,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.CENTER
            )
        )

        statusView = TextView(this).apply {
            text = "准备游戏运行时"
            setTextColor(primaryTextColor())
            textSize = 18f
            gravity = Gravity.CENTER
            typeface = android.graphics.Typeface.DEFAULT_BOLD
            includeFontPadding = false
        }
        content.addView(statusView, LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT
        ))

        detailView = TextView(this).apply {
            text = ""
            setTextColor(secondaryTextColor())
            textSize = 12f
            gravity = Gravity.CENTER
            includeFontPadding = false
        }
        content.addView(detailView, LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT
        ).apply { topMargin = dp(8) })

        progressBar = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            progressTintList = android.content.res.ColorStateList.valueOf(accentColor)
            progressBackgroundTintList = android.content.res.ColorStateList.valueOf(withAlpha(accentColor, 42))
        }
        content.addView(progressBar, LinearLayout.LayoutParams(
            (resources.displayMetrics.widthPixels - dp(120)).coerceIn(dp(200), dp(420)),
            dp(5)
        ).apply { topMargin = dp(20) })

        logScroll = ScrollView(this).apply {
            isFillViewport = false
            background = GradientDrawable().apply {
                cornerRadius = dp(8).toFloat()
                setColor(if (isDarkMode()) Color.argb(238, 24, 26, 28) else Color.WHITE)
                setStroke(dp(1), withAlpha(accentColor, if (isDarkMode()) 88 else 120))
            }
        }
        logView = TextView(this).apply {
            setTextColor(secondaryTextColor())
            textSize = 11f
            includeFontPadding = false
            typeface = android.graphics.Typeface.MONOSPACE
        }
        logScroll.addView(logView, FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT
        ))
        content.addView(logScroll, LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            dp(150)
        ).apply { topMargin = dp(22) })

        returnButton = Button(this).apply {
            setText("返回启动器")
            isVisible(false)
            backgroundTintList = android.content.res.ColorStateList.valueOf(accentColor)
            setTextColor(Color.WHITE)
            setOnClickListener { returnToLauncher() }
        }
        content.addView(returnButton, LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT,
            ViewGroup.LayoutParams.WRAP_CONTENT
        ).apply { topMargin = dp(18) })

        setContentView(root)
    }

    private fun View.isVisible(visible: Boolean) {
        visibility = if (visible) View.VISIBLE else View.GONE
    }

    private fun withAlpha(color: Int, alpha: Int): Int = Color.argb(
        alpha.coerceIn(0, 255),
        Color.red(color),
        Color.green(color),
        Color.blue(color)
    )

    private fun isDarkMode(): Boolean =
        (resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK) ==
            android.content.res.Configuration.UI_MODE_NIGHT_YES

    // ------------------------------------------------------------------ 流程

    private fun startPreparingAfterFirstDraw() {
        val content = window.decorView
        content.viewTreeObserver.addOnPreDrawListener(object : android.view.ViewTreeObserver.OnPreDrawListener {
            override fun onPreDraw(): Boolean {
                if (content.viewTreeObserver.isAlive) {
                    content.viewTreeObserver.removeOnPreDrawListener(this)
                }
                content.post { startPreparingOnce() }
                return true
            }
        })
        content.postDelayed({ startPreparingOnce() }, FIRST_FRAME_FALLBACK_MS)
    }

    private fun startPreparingOnce() {
        if (!preparingStarted.compareAndSet(false, true)) return
        executor.execute {
            try {
                val game = CopperGameInstance.fromIntent(applicationContext, intent)
                trace?.milestone("实例已解析: ${game.name}")
                CopperGameInstance.verifyPackage(applicationContext, game)
                trace?.milestone("包名校验通过")

                onProgress(6, "准备游戏文件", game.baseApk.name)
                val manager = CopperGamePackageManager.getInstance(applicationContext, game, this)
                onProgress(40, "准备启动", game.versionCode)

                val gameIntent = Intent(intent).setClass(this, CopperGameActivity::class.java)
                CopperGameRuntimePreparer.fillLaunchExtras(applicationContext, gameIntent, game, manager)
                CopperGameRuntimePreparer.configureFirebaseExtras(gameIntent, manager)
                trace?.mark("启动 Intent 已填充")

                onProgress(44, "加载原生库")
                CopperGameRuntimePreparer.loadNativeLibraries(game, manager, this)
                trace?.milestone("原生库加载完成")

                CopperGameSession.set(game, manager)
                onProgress(100, "正在进入游戏")
                mainHandler.post { enterGame(gameIntent) }
            } catch (error: Throwable) {
                CopperGameRuntimePreparer.logFailure("运行时准备", error)
                mainHandler.post { showFailure(error) }
            }
        }
    }

    private fun enterGame(gameIntent: Intent) {
        if (enteringGame || returningToLauncher.get() || isFinishing || isDestroyed) return
        enteringGame = true
        trace?.milestone("进入游戏 Activity")
        startActivity(gameIntent)
        overridePendingTransition(0, 0)
        finish()
        overridePendingTransition(0, 0)
    }

    private fun showFailure(error: Throwable) {
        if (isFinishing || isDestroyed) return
        val message = error.message ?: error.javaClass.simpleName
        statusView.text = "启动失败"
        detailView.text = message
        progressBar.progress = 100
        appendLog("启动失败")
        appendLog(message)
        returnButton.visibility = View.VISIBLE
        trace?.error("准备失败", message)
    }

    override fun onProgress(progress: Int, status: String, detail: String?) {
        mainHandler.post {
            if (isFinishing || isDestroyed) return@post
            currentProgress = progress.coerceIn(0, 100)
            progressBar.progress = currentProgress
            statusView.text = status
            detailView.text = detail.orEmpty()
        }
    }

    override fun onLog(message: String) {
        mainHandler.post { appendLog(message) }
    }

    private fun appendLog(message: String) {
        if (isFinishing || isDestroyed) return
        if (message == lastLogMessage) return
        lastLogMessage = message
        visibleLog.addLast("[${timeFormat.format(Date())}] $message")
        while (visibleLog.size > MAX_LOG_LINES) visibleLog.removeFirst()
        logView.text = visibleLog.joinToString(separator = "\n", postfix = "\n")
        logScroll.post { logScroll.fullScroll(ScrollView.FOCUS_DOWN) }
    }

    private fun returnToLauncher() {
        if (!returningToLauncher.compareAndSet(false, true)) return
        CopperGameSession.clear()
        CopperGamePackageManager.reset()
        finish()
        overridePendingTransition(0, 0)
    }

    @Deprecated("Deprecated in Java")
    @Suppress("DEPRECATION")
    override fun onBackPressed() {
        returnToLauncher()
    }

    override fun onResume() {
        super.onResume()
        hideSystemUi()
    }

    override fun onDestroy() {
        executor.shutdownNow()
        mainHandler.removeCallbacksAndMessages(null)
        super.onDestroy()
    }

    private fun hideSystemUi() {
        val decor = window.decorView
        window.statusBarColor = Color.TRANSPARENT
        window.navigationBarColor = Color.TRANSPARENT
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            window.isStatusBarContrastEnforced = false
            window.isNavigationBarContrastEnforced = false
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.setDecorFitsSystemWindows(false)
            decor.windowInsetsController?.let { controller ->
                controller.systemBarsBehavior =
                    android.view.WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                controller.hide(android.view.WindowInsets.Type.statusBars() or android.view.WindowInsets.Type.navigationBars())
            }
        }
        @Suppress("DEPRECATION")
        decor.systemUiVisibility =
            View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
                View.SYSTEM_UI_FLAG_FULLSCREEN or
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE
    }

    private companion object {
        const val FIRST_FRAME_FALLBACK_MS = 240L
        const val MAX_LOG_LINES = 60
    }
}
