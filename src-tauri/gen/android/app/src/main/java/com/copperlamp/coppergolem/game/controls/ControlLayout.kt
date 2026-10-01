package com.copperlamp.coppergolem.game.controls

import org.json.JSONObject
import java.io.File

/** 控件类型。取值与 Rust `home::controls` 的常量逐字相同。 */
enum class ControlKind(val wire: String) {
    /** 四向方向键（前进 / 后退 / 左 / 右）。 */
    DPad("dpad"),

    /** 摇杆（同样映射到方向动作，操作手感不同）。 */
    Joystick("joystick"),

    /** 单动作按钮。 */
    Button("button"),

    /** 视角区：拖动即转动视角。 */
    Look("look");

    companion object {
        fun fromWire(value: String): ControlKind? = entries.firstOrNull { it.wire == value }
    }
}

/**
 * 控件绑定的**游戏动作**。
 *
 * 刻意不出现按键码：按键码是平台细节，写进布局会把用户资产绑死在某个平台上。
 * 动作 → 按键码的映射在 [ActionKeys]。
 */
enum class ControlAction(val wire: String) {
    Forward("forward"),
    Back("back"),
    Left("left"),
    Right("right"),
    Jump("jump"),
    Sneak("sneak"),
    Sprint("sprint"),
    Inventory("inventory"),
    Drop("drop"),
    Attack("attack"),
    Use("use"),
    ToggleFly("toggle_fly"),
    Chat("chat"),
    Menu("menu");

    val isDirectional: Boolean
        get() = this == Forward || this == Back || this == Left || this == Right

    companion object {
        fun fromWire(value: String): ControlAction? = entries.firstOrNull { it.wire == value }
    }
}

/**
 * 归一化矩形（0..1）。
 *
 * 与像素无关：换机、旋转、分屏后同一个布局都能还原。像素换算在
 * [ControlOverlayView] 里按当前视图尺寸做一次。
 */
data class ControlRect(val x: Float, val y: Float, val w: Float, val h: Float) {
    fun left(px: Float): Float = x * px

    fun top(py: Float): Float = y * py

    fun width(px: Float): Float = w * px

    fun height(py: Float): Float = h * py

    fun contains(px: Float, py: Float, width: Float, height: Float, x: Float, y: Float): Boolean =
        x >= left(width) && x <= left(width) + this.width(width) &&
            y >= top(height) && y <= top(height) + this.height(height)
}

/** 一个控件。 */
data class Control(
    val id: String,
    val kind: ControlKind,
    val rect: ControlRect,
    val label: String,
    val action: ControlAction?,
    val opacity: Float,
    val visible: Boolean
)

/**
 * 一份触控布局，读自 `<实例目录>/controls.json`。
 *
 * **只读方**：文件由 Rust 侧（`home::controls`）校验并原子写入，这里不做业务校验，
 * 只在数据不可用时退回 [default] 并说明原因——玩家因此至少还能进游戏。
 * 这与「Rust 管文件与事实、Kotlin 管 Android 机制」的分工一致。
 */
data class ControlLayout(val controls: List<Control>) {

    companion object {
        /** 布局文件名（与 Rust `home::controls::CONTROLS_FILE` 相同）。 */
        const val FILE_NAME = "controls.json"

        /**
         * 默认布局：与 Rust `ControlLayout::default` 保持**同构**（不是同文本）。
         *
         * 两处都写一份是因为文件缺失时 Rust 已经把默认值返回给了前端，但游戏进程
         * 可能在完全没有经过前端的情况下启动（直接深链）；此时 Kotlin 必须能自己
         * 给出一个可玩的布局，而不是空屏。
         */
        fun default(): ControlLayout = ControlLayout(
            listOf(
                Control(
                    "dpad", ControlKind.DPad, ControlRect(0.04f, 0.55f, 0.28f, 0.40f),
                    "", null, 0.45f, true
                ),
                Control(
                    "jump", ControlKind.Button, ControlRect(0.845f, 0.62f, 0.13f, 0.18f),
                    "", ControlAction.Jump, 0.5f, true
                ),
                Control(
                    "sneak", ControlKind.Button, ControlRect(0.72f, 0.68f, 0.11f, 0.15f),
                    "", ControlAction.Sneak, 0.5f, true
                ),
                Control(
                    "inventory", ControlKind.Button, ControlRect(0.02f, 0.06f, 0.09f, 0.13f),
                    "", ControlAction.Inventory, 0.5f, true
                ),
                Control(
                    "menu", ControlKind.Button, ControlRect(0.13f, 0.06f, 0.08f, 0.12f),
                    "", ControlAction.Menu, 0.5f, true
                )
            )
        )

        /**
         * 读取实例目录下的布局；任何问题都退回默认布局并把原因写进 [loadReason]。
         *
         * 返回值恒非空：触控层是「能不能玩」的必要条件，不能因为一份坏 JSON 让
         * 玩家卡在没法操作的画面上。
         */
        fun load(instanceDir: File): LoadResult {
            val file = File(instanceDir, FILE_NAME)
            if (!file.isFile) {
                return LoadResult(default(), "布局文件不存在，使用默认布局")
            }
            val raw = try {
                file.readText()
            } catch (error: Exception) {
                return LoadResult(default(), "布局读取失败: ${error.message}")
            }
            return parse(raw)
        }

        /** 解析 JSON 文本（与 Rust 侧字段名逐字对应）。 */
        fun parse(raw: String): LoadResult {
            if (raw.isBlank()) {
                return LoadResult(default(), "布局文件为空，使用默认布局")
            }
            val json = try {
                JSONObject(raw)
            } catch (error: Exception) {
                return LoadResult(default(), "布局 JSON 解析失败: ${error.message}")
            }
            val version = json.optInt("version", 0)
            if (version > 1) {
                // 更高版本可能带来本版本不认识的语义，按旧理解渲染会静默走样；
                // 退回默认并说明，玩家至少能玩，也能看懂为什么布局变了。
                return LoadResult(default(), "布局版本 $version 高于本版本支持，使用默认布局")
            }
            val array = json.optJSONArray("controls")
                ?: return LoadResult(default(), "布局缺少 controls 字段，使用默认布局")

            val parsed = ArrayList<Control>(array.length())
            val seen = HashSet<String>()
            for (index in 0 until array.length()) {
                val item = array.optJSONObject(index) ?: continue
                val control = parseControl(item) ?: continue
                if (!seen.add(control.id)) continue
                parsed.add(control)
            }
            if (parsed.isEmpty()) {
                return LoadResult(default(), "布局没有任何可用控件，使用默认布局")
            }
            // 视角区不写进文件也能工作（整屏兜底），因此不强制要求存在。
            return LoadResult(ControlLayout(parsed), null)
        }

        private fun parseControl(item: JSONObject): Control? {
            val id = item.optString("id").trim()
            if (id.isEmpty()) return null
            val kind = ControlKind.fromWire(item.optString("kind")) ?: return null
            val rectJson = item.optJSONObject("rect") ?: return null
            val rect = ControlRect(
                rectJson.optDouble("x", 0.0).toFloat(),
                rectJson.optDouble("y", 0.0).toFloat(),
                rectJson.optDouble("w", 0.1).toFloat(),
                rectJson.optDouble("h", 0.1).toFloat()
            ).normalized()
            val actionWire = item.optString("action")
            val action = if (actionWire.isEmpty()) null else ControlAction.fromWire(actionWire)
            return Control(
                id = id,
                kind = kind,
                rect = rect,
                label = item.optString("label"),
                // 视角区不接受动作绑定（与 Rust 侧同规则）。
                action = if (kind == ControlKind.Look) null else action,
                opacity = item.optDouble("opacity", 0.5).toFloat().coerceIn(0.1f, 1f),
                visible = item.optBoolean("visible", true)
            )
        }

        private fun ControlRect.normalized(): ControlRect {
            val nw = w.coerceIn(0.02f, 1f)
            val nh = h.coerceIn(0.02f, 1f)
            return ControlRect(
                x.coerceIn(0f, 1f - nw),
                y.coerceIn(0f, 1f - nh),
                nw,
                nh
            )
        }
    }
}

/** 布局装载结果：布局恒可用，[reason] 非空表示走了兜底（写 logcat 用）。 */
data class LoadResult(val layout: ControlLayout, val reason: String?)
