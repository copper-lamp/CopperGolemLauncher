package com.copperlamp.coppergolem.game.controls

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.os.SystemClock
import android.util.Log
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View

/**
 * 把「控件被按下」翻译成游戏能收到的输入。
 *
 * ## 为什么走 Java 输入路径而不是注入 native
 *
 * 参考实现（LeviLaunchroid 的 `pojav_controls`）的按键 / 鼠标 / 视角全部经
 * `libinbuiltmods.so` 的 native 方法注入（`PojavControlsMod.java:24-28`）。该库在
 * 仓库里**没有源码**、许可是 LGPL-3.0，与铜傀儡不可复用（见
 * docs/安卓端能力差距与优先级.md P1-1 与本次调研结论）。
 *
 * 而 AGDK `GameActivity`（4.4.2）的输入路径是纯 Java：触摸经
 * `GameActivity$InputEnabledSurfaceView` 的 `OnTouchListener` → `processMotionEvent`
 * → native；按键落在 `GameActivity.onKeyDown/onKeyUp` → native。因此子类**直接调用
 * 这些入口**即可把合成事件送进游戏，无需任何 native 库。
 *
 * ## 关键约束
 *
 * 1. **绝不复用 `dispatchTouchEvent` 注入**：同进程里再走一遍 Activity 分发会重新
 *    命中触控层自身，形成「注入 → 又被自己吃掉 → 再注入」的回环。注入必须绕过
 *    覆盖层，直连游戏视图。
 * 2. **按键走 `onKeyDown/onKeyUp` 而不是 `dispatchKeyEvent`**：后者会先给 Mojang
 *    `nativeKeyHandler` 过一遍，那是**物理按键**通道；用 Activity 的 onKey* 语义更
 *    接近真实输入，也避免与 `CopperGameActivity` 自己的分发逻辑互相递归。
 * 3. 合成 `KeyEvent` 带 `FLAG_SOFT_KEYBOARD`，让游戏侧能区分「硬件按键」与「触控层
 *    合成输入」。
 */
interface GameInputSink {
    /** 把一个合成按键事件交给游戏。 */
    fun sendKey(keyCode: Int, down: Boolean)

    /** 把一次触摸交给游戏（视角拖动 / 屏幕点击）。实现方必须绕过触控覆盖层。 */
    fun sendTouch(event: MotionEvent)

    /** 把一次相对鼠标移动（视角增量）交给游戏。 */
    fun sendRelativeMotion(dx: Float, dy: Float)

    /**
     * 当前是否正在把触摸注入给游戏。
     *
     * 覆盖层用它**斩断回环**：合成事件会经窗口分发链回到 Activity，若此时还让
     * 覆盖层处理一次，就会「注入 → 又被自己吃掉 → 再注入」。
     */
    fun isInjectingTouch(): Boolean
}

/**
 * 动作 → Android 按键码。
 *
 * Bedrock 的键盘映射是固定的（官方客户端即按此接受输入）。这里只做「动作 → 码」
 * 的翻译，不做组合键判定。
 */
object ActionKeys {
    fun keyCodeOf(action: ControlAction): Int = when (action) {
        ControlAction.Forward -> KeyEvent.KEYCODE_W
        ControlAction.Back -> KeyEvent.KEYCODE_S
        ControlAction.Left -> KeyEvent.KEYCODE_A
        ControlAction.Right -> KeyEvent.KEYCODE_D
        ControlAction.Jump -> KeyEvent.KEYCODE_SPACE
        ControlAction.Sneak -> KeyEvent.KEYCODE_SHIFT_LEFT
        ControlAction.Sprint -> KeyEvent.KEYCODE_CTRL_LEFT
        ControlAction.Inventory -> KeyEvent.KEYCODE_E
        ControlAction.Drop -> KeyEvent.KEYCODE_Q
        ControlAction.Attack -> KeyEvent.KEYCODE_BUTTON_R1
        ControlAction.Use -> KeyEvent.KEYCODE_BUTTON_L1
        ControlAction.ToggleFly -> KeyEvent.KEYCODE_F
        ControlAction.Chat -> KeyEvent.KEYCODE_T
        ControlAction.Menu -> KeyEvent.KEYCODE_ESCAPE
    }
}

/**
 * 在宿主 Activity 上落地 [GameInputSink]。
 *
 * 持有 Activity 与 AGDK 游戏视图；两者由宿主在 `onCreate` 后注入。[gameSurface] 为
 * `null` 时回退到窗口 DecorView——AGDK 把游戏 SurfaceView 放在一个全屏 FrameLayout
 * 里，DecorView 分发同样能命中它。
 */
class InjectingInputSink(
    private val activity: android.app.Activity,
    private val gameSurface: View?
) : GameInputSink {

    /** 已按下的键：合成事件的「首次按下 / 重复」与 `downTime` 靠它维护。 */
    private val pressed = HashSet<Int>()

    private var downTime = 0L

    /**
     * 注入中的触摸。
     *
     * `@Volatile`：`sendTouch` 与 `dispatchTouchEvent` 都在主线程，但覆盖层也可能
     * 从其它线程被问（例如后台释放按键），保守加一层可见性保证。
     */
    @Volatile
    private var injectingTouch = false

    override fun sendKey(keyCode: Int, down: Boolean) {
        if (down) {
            // 已按下就不重复注入：游戏侧本来就是「持续按住」语义。
            if (!pressed.add(keyCode)) return
            if (pressed.size == 1) downTime = SystemClock.uptimeMillis()
        } else {
            if (!pressed.remove(keyCode)) return
        }

        val now = SystemClock.uptimeMillis()
        val start = if (downTime == 0L) now else downTime
        val event = KeyEvent(
            start,
            now,
            if (down) KeyEvent.ACTION_DOWN else KeyEvent.ACTION_UP,
            keyCode,
            0,
            0,
            -1,
            0,
            KeyEvent.FLAG_SOFT_KEYBOARD
        )
        try {
            // AGDK 在这两个方法里直通 native；不要换成 dispatchKeyEvent（见类文档）。
            if (down) {
                activity.onKeyDown(keyCode, event)
            } else {
                activity.onKeyUp(keyCode, event)
            }
        } catch (error: Throwable) {
            Log.w(TAG, "注入按键 $keyCode（down=$down）失败: ${error.message}")
        } finally {
            if (pressed.isEmpty()) downTime = 0L
        }
    }

    override fun sendTouch(event: MotionEvent) {
        val target = gameSurface ?: activity.window?.decorView
        if (target == null) {
            Log.w(TAG, "没有可用的游戏视图，触摸注入被丢弃")
            return
        }
        // 标志必须覆盖整个 dispatch：事件会同步穿过 DecorView → 游戏 SurfaceView，
        // 期间若回到 window 分发链，覆盖层据此放行。
        injectingTouch = true
        try {
            target.dispatchTouchEvent(event)
        } finally {
            injectingTouch = false
        }
    }

    override fun isInjectingTouch(): Boolean = injectingTouch

    override fun sendRelativeMotion(dx: Float, dy: Float) {
        val now = SystemClock.uptimeMillis()
        val event = MotionEvent.obtain(now, now, MotionEvent.ACTION_HOVER_MOVE, 0f, 0f, 0)
        event.source = InputDevice.SOURCE_MOUSE_RELATIVE
        event.setAxisValue(MotionEvent.AXIS_RELATIVE_X, dx)
        event.setAxisValue(MotionEvent.AXIS_RELATIVE_Y, dy)
        try {
            sendTouch(event)
        } finally {
            event.recycle()
        }
    }

    /** 释放所有仍按下的键（覆盖层被移除 / 活动销毁时调用）。 */
    fun releaseAll() {
        for (keyCode in pressed.toList()) {
            sendKey(keyCode, false)
        }
    }

    private companion object {
        const val TAG = "CopperGameInput"
    }
}

/**
 * 触控覆盖层。
 *
 * 职责：按布局渲染控件、把触摸翻译成 [GameInputSink] 调用。
 * 不负责注入（见 [InjectingInputSink]），也不负责布局持久化（归 Rust）。
 *
 * 事件流向（由 `CopperGameActivity.dispatchTouchEvent` 驱动）：
 *
 * ```text
 * 真实触摸 → Activity.dispatchTouchEvent
 *              ├─ 先给本视图（onTouchEvent）
 *              │    ├─ 命中控件 → 翻译成按键 / 视角增量，return true（吃掉）
 *              │    └─ 未命中   → return false（不吃）
 *              └─ 未被吃掉 → 转发给游戏视图（真实触摸直接进游戏）
 * ```
 *
 * 覆盖层因此**不遮挡**未被控件占用的区域，玩家仍能直接点屏幕挖方块、转视角。
 */
class ControlOverlayView(
    context: Context,
    private val input: GameInputSink
) : View(context) {

    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val fillPaint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val strokePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
    }
    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        textAlign = Paint.Align.CENTER
    }

    private var layout: ControlLayout = ControlLayout.default()

    /** pointerId → 正在操作的控件 id。 */
    private val activePointers = HashMap<Int, String>()

    /** 控件 id → 当前方向动作（摇杆 / 方向键的按下方向）。 */
    private val pressedActions = HashMap<String, ControlAction>()

    /** 视角拖动：pointerId → 上次坐标。 */
    private val lookAnchors = HashMap<Int, FloatArray>()

    /** 每个控件当前是否处于「按下」视觉态。 */
    private val pressedVisual = HashSet<String>()

    /** 编辑模式：只显示轮廓、不注入输入（供 Vue 侧预览对齐用）。 */
    var editing: Boolean = false
        set(value) {
            field = value
            if (value) {
                releaseAll()
            }
            invalidate()
        }

    init {
        // 覆盖层自己不抢焦点：焦点要留给游戏视图，否则真实按键会落到这里。
        isFocusable = false
        isFocusableInTouchMode = false
        setWillNotDraw(false)
    }

    /** 替换布局（游戏启动时调用一次；编辑态由宿主重新挂载）。 */
    fun setLayout(next: ControlLayout) {
        releaseAll()
        layout = next
        invalidate()
    }

    /** 释放所有按下的按键（移除覆盖层 / 进入后台时调用）。 */
    fun releaseAll() {
        for ((id, action) in pressedActions) {
            input.sendKey(ActionKeys.keyCodeOf(action), false)
            pressedVisual.remove(id)
        }
        pressedActions.clear()
        lookAnchors.clear()
        activePointers.clear()
        invalidate()
    }

    // ------------------------------------------------------------------ 触摸

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (input.isInjectingTouch()) {
            // 这是我们自己刚注入的事件绕回来了：必须放行，否则成环。
            return false
        }
        if (editing) {
            // 编辑态不吃事件：让真实触摸透到游戏，方便边看边调。
            return false
        }

        var handled = false
        val action = event.actionMasked
        when (action) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                val index = event.actionIndex
                val pointerId = event.getPointerId(index)
                val x = event.getX(index)
                val y = event.getY(index)
                val control = controlAt(x, y)
                if (control != null) {
                    activePointers[pointerId] = control.id
                    if (control.kind == ControlKind.Look) {
                        lookAnchors[pointerId] = floatArrayOf(x, y)
                    } else {
                        applyDirection(control, x, y)
                    }
                    pressedVisual.add(control.id)
                    handled = true
                }
            }

            MotionEvent.ACTION_MOVE -> {
                for (index in 0 until event.pointerCount) {
                    val pointerId = event.getPointerId(index)
                    val id = activePointers[pointerId] ?: continue
                    val control = controlById(id) ?: continue
                    val x = event.getX(index)
                    val y = event.getY(index)
                    if (control.kind == ControlKind.Look) {
                        val anchor = lookAnchors[pointerId]
                        if (anchor != null) {
                            // 视角增量：只在超过阈值时下发，避免高频抖动刷爆输入队列。
                            val dx = x - anchor[0]
                            val dy = y - anchor[1]
                            if (kotlin.math.abs(dx) >= LOOK_STEP_PX || kotlin.math.abs(dy) >= LOOK_STEP_PX) {
                                input.sendRelativeMotion(dx * LOOK_SENSITIVITY, dy * LOOK_SENSITIVITY)
                                anchor[0] = x
                                anchor[1] = y
                            }
                        }
                    } else {
                        applyDirection(control, x, y)
                    }
                    handled = true
                }
            }

            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP, MotionEvent.ACTION_CANCEL -> {
                if (action == MotionEvent.ACTION_CANCEL) {
                    releaseAll()
                    return true
                }
                val index = event.actionIndex
                val pointerId = event.getPointerId(index)
                val id = activePointers.remove(pointerId)
                if (id != null) {
                    lookAnchors.remove(pointerId)
                    releaseControl(id)
                    handled = true
                }
            }
        }

        if (handled) invalidate()
        return handled
    }

    /** 命中测试：从后往前（后加的控件在上层）。 */
    private fun controlAt(x: Float, y: Float): Control? {
        for (index in layout.controls.indices.reversed()) {
            val control = layout.controls[index]
            if (!control.visible) continue
            if (control.rect.contains(0f, 0f, width.toFloat(), height.toFloat(), x, y)) {
                return control
            }
        }
        return null
    }

    private fun controlById(id: String): Control? = layout.controls.firstOrNull { it.id == id }

    /**
     * 把触点位置翻译成方向动作并下发（方向键四象限 / 摇杆八向）。
     *
     * 只在方向变化时下发：同一方向上持续按住靠「不发 up」表达，不需要重复 down。
     */
    private fun applyDirection(control: Control, x: Float, y: Float) {
        val left = control.rect.left(width.toFloat())
        val top = control.rect.top(height.toFloat())
        val cx = left + control.rect.width(width.toFloat()) / 2f
        val cy = top + control.rect.height(height.toFloat()) / 2f
        val dx = x - cx
        val dy = y - cy

        // 死区：手指落在中心附近不触发方向，避免误触。
        val deadZone = control.rect.width(width.toFloat()) * DEAD_ZONE_RATIO
        val direction: ControlAction? = if (kotlin.math.abs(dx) < deadZone && kotlin.math.abs(dy) < deadZone) {
            null
        } else if (control.kind == ControlKind.DPad) {
            // 四向：取主轴，避免斜向同时触发两个方向（四向键的物理几何就是十字）。
            if (kotlin.math.abs(dx) > kotlin.math.abs(dy)) {
                if (dx > 0) ControlAction.Right else ControlAction.Left
            } else {
                if (dy > 0) ControlAction.Back else ControlAction.Forward
            }
        } else {
            // 摇杆：八向，两个方向可以同时按下（与真实 WASD 一致）。
            if (dy < -deadZone) ControlAction.Forward else if (dy > deadZone) ControlAction.Back else null
        }

        val horizontal: ControlAction? = if (control.kind == ControlKind.Joystick) {
            if (dx < -deadZone) ControlAction.Left else if (dx > deadZone) ControlAction.Right else null
        } else {
            null
        }

        val wanted = setOfNotNull(direction, horizontal)
        val current = pressedActions[control.id]

        // 方向变了（或离开死区）才动输入。
        if (current != null && current !in wanted) {
            input.sendKey(ActionKeys.keyCodeOf(current), false)
            pressedActions.remove(control.id)
        }
        val next = wanted.firstOrNull()
        if (next != null && next != pressedActions[control.id]) {
            input.sendKey(ActionKeys.keyCodeOf(next), true)
            pressedActions[control.id] = next
        }
    }

    private fun releaseControl(id: String) {
        pressedActions.remove(id)?.let { action ->
            input.sendKey(ActionKeys.keyCodeOf(action), false)
        }
        pressedVisual.remove(id)
    }

    // ------------------------------------------------------------------ 渲染

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        for (control in layout.controls) {
            if (!control.visible) continue
            when (control.kind) {
                ControlKind.DPad -> drawDPad(canvas, control)
                ControlKind.Joystick -> drawJoystick(canvas, control)
                ControlKind.Look -> drawLookArea(canvas, control)
                ControlKind.Button -> drawButton(canvas, control)
            }
        }
    }

    private fun accent(): Int = ACCENT

    private fun drawDPad(canvas: Canvas, control: Control) {
        val box = boxOf(control)
        val alpha = alphaOf(control)
        strokePaint.color = withAlpha(accent(), 140)
        strokePaint.strokeWidth = dp(1.5f)
        fillPaint.color = withAlpha(Color.BLACK, (60 * alpha).toInt())

        // 十字：四臂各画一个方角，中心留空，视觉上能读出「方向」。
        val armW = box.width() / 3f
        val armH = box.height() / 3f
        val pressed = pressedVisual.contains(control.id)
        val colors = if (pressed) withAlpha(accent(), 90) else fillPaint.color

        fillPaint.color = colors
        canvas.drawRect(RectF(box.left + armW, box.top, box.right - armW, box.top + armH), fillPaint)
        canvas.drawRect(RectF(box.left + armW, box.bottom - armH, box.right - armW, box.bottom), fillPaint)
        canvas.drawRect(RectF(box.left, box.top + armH, box.left + armW, box.bottom - armH), fillPaint)
        canvas.drawRect(RectF(box.right - armW, box.top + armH, box.right, box.bottom - armH), fillPaint)
        canvas.drawRect(box, strokePaint)
    }

    private fun drawJoystick(canvas: Canvas, control: Control) {
        val box = boxOf(control)
        val alpha = alphaOf(control)
        val cx = box.centerX()
        val cy = box.centerY()
        val radius = kotlin.math.min(box.width(), box.height()) / 2f

        strokePaint.color = withAlpha(accent(), 140)
        strokePaint.strokeWidth = dp(1.5f)
        fillPaint.color = withAlpha(Color.BLACK, (60 * alpha).toInt())
        canvas.drawCircle(cx, cy, radius, fillPaint)
        canvas.drawCircle(cx, cy, radius, strokePaint)

        // 拇指：按下的方向偏移一点，给出「真的动了」的反馈。
        val action = pressedActions[control.id]
        val offsetX = when (action) {
            ControlAction.Left -> -radius * 0.45f
            ControlAction.Right -> radius * 0.45f
            else -> 0f
        }
        val offsetY = when (action) {
            ControlAction.Forward -> -radius * 0.45f
            ControlAction.Back -> radius * 0.45f
            else -> 0f
        }
        fillPaint.color = withAlpha(accent(), if (action == null) 90 else 170)
        canvas.drawCircle(cx + offsetX, cy + offsetY, radius * 0.42f, fillPaint)
    }

    private fun drawLookArea(canvas: Canvas, control: Control) {
        // 视角区平时不可见（否则整屏被一层灰盖住）；编辑态画虚线轮廓。
        if (!editing) return
        strokePaint.color = withAlpha(accent(), 110)
        strokePaint.strokeWidth = dp(1f)
        canvas.drawRect(boxOf(control), strokePaint)
    }

    private fun drawButton(canvas: Canvas, control: Control) {
        val box = boxOf(control)
        val alpha = alphaOf(control)
        val pressed = pressedVisual.contains(control.id)

        fillPaint.color = withAlpha(accent(), if (pressed) 170 else (70 * alpha).toInt().coerceAtLeast(40))
        strokePaint.color = withAlpha(accent(), 150)
        strokePaint.strokeWidth = dp(1.5f)
        val radius = dp(8f)
        canvas.drawRoundRect(box, radius, radius, fillPaint)
        canvas.drawRoundRect(box, radius, radius, strokePaint)

        val label = labelOf(control)
        if (label.isNotEmpty()) {
            textPaint.color = withAlpha(Color.WHITE, 220)
            textPaint.textSize = kotlin.math.min(box.height() * 0.34f, dp(16f))
            canvas.drawText(label, box.centerX(), box.centerY() - (textPaint.descent() + textPaint.ascent()) / 2f, textPaint)
        }
    }

    private fun boxOf(control: Control): RectF {
        val w = width.toFloat()
        val h = height.toFloat()
        return RectF(
            control.rect.left(w),
            control.rect.top(h),
            control.rect.left(w) + control.rect.width(w),
            control.rect.top(h) + control.rect.height(h)
        )
    }

    private fun alphaOf(control: Control): Float = if (editing) 1f else control.opacity

    /** 按钮文案：布局给的 label 优先，否则按动作取默认英文缩写。 */
    private fun labelOf(control: Control): String {
        if (control.label.isNotBlank()) return control.label
        return when (control.action) {
            ControlAction.Jump -> "JUMP"
            ControlAction.Sneak -> "SNEAK"
            ControlAction.Sprint -> "SPRINT"
            ControlAction.Inventory -> "BAG"
            ControlAction.Drop -> "DROP"
            ControlAction.Attack -> "ATK"
            ControlAction.Use -> "USE"
            ControlAction.ToggleFly -> "FLY"
            ControlAction.Chat -> "CHAT"
            ControlAction.Menu -> "MENU"
            else -> ""
        }
    }

    private fun dp(value: Float): Float = value * resources.displayMetrics.density

    private fun withAlpha(color: Int, alpha: Int): Int =
        Color.argb(alpha.coerceIn(0, 255), Color.red(color), Color.green(color), Color.blue(color))

    private companion object {
        /** 与铜内核主题的强调色一致（琥珀），避免与启动器 UI 割裂。 */
        const val ACCENT = 0xFFC67C2E.toInt()

        /** 死区：占控件宽度的比例。 */
        const val DEAD_ZONE_RATIO = 0.18f

        /** 视角下发阈值（像素）与灵敏度。 */
        const val LOOK_STEP_PX = 4f
        const val LOOK_SENSITIVITY = 1.6f
    }
}
