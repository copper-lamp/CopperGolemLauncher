//! 屏幕触控层的布局事实源：`<版本目录>/controls.json`。
//!
//! **职责边界**：本模块只定义布局的**数据契约**与读写，不含任何安卓代码。
//! 渲染与输入注入在 Kotlin（`game/controls/`），编辑界面在 Vue（模块设置页）。
//! 这与 [安卓适配-LeviLaunchroid调研与三功能方案](../../../docs/安卓适配-LeviLaunchroid调研与三功能方案.md)
//! 2.3 的划分一致：**Rust 管文件与事实，Kotlin 管 Android 机制，UI 全走 Vue**。
//!
//! 为什么不是 Kotlin 自己存：布局是用户资产，需要（a）跨实例隔离、（b）可校验、
//! （c）可被 Vue 编辑器读写、（d）在游戏进程崩溃后依然完整。放在版本目录里，
//! 用「临时文件 + rename」原子写，天然满足这四条；Kotlin 侧只读不写。
//!
//! 文件格式（与 Kotlin `ControlLayout.kt` 逐字对应，改一处必须改两处）：

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::state::KernelContext;

use super::meta::resolve_version_dir;

/// 布局文件名（版本目录内）。
pub const CONTROLS_FILE: &str = "controls.json";

/// 当前 schema 版本。
///
/// 升级规则：读取时 `version` 小于当前值按缺失字段补默认；**大于**当前值直接
/// 报错——用新版本写的布局可能有本版本不认识的语义，按旧理解渲染会静默走样。
pub const SCHEMA_VERSION: u32 = 1;

/// 坐标空间：布局用「屏幕比例」表达，不存像素。
///
/// 存像素的布局在换机、旋转、分屏后必然错位；比例坐标在任何屏幕上都能还原。
/// 这是本模块与参考实现（PojavLauncher 血统的 `dynamicX/dynamicY` 表达式）在
/// 语义上的关键差别：我们不引入表达式求值器，只用 0..1 的比例。
pub const COORDINATE_SPACE: &str = "fraction";

/// 控件类型（与 Kotlin 侧字符串常量一一对应）。
pub const KIND_DPAD: &str = "dpad";
pub const KIND_JOYSTICK: &str = "joystick";
pub const KIND_BUTTON: &str = "button";
pub const KIND_LOOK: &str = "look";

/// 控件绑定的动作（游戏语义，不是按键码）。
///
/// 用动作而非按键码是刻意的：**按键码是平台细节**（Windows 的 GLFW 码、Android 的
/// `KeyEvent` 码、Bedrock 内部码各不相同），把它写进用户布局会把布局绑死在某个
/// 平台上。动作到按键的映射由 Kotlin 侧的能力表完成。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    /// 前进（前进方向，摇杆/方向键向上）
    Forward,
    Back,
    Left,
    Right,
    /// 跳跃（默认绑定到「上移」类按键）
    Jump,
    /// 潜行 / 下蹲
    Sneak,
    /// 疾跑修饰键（与前进组合触发冲刺）
    Sprint,
    /// 打开 / 关闭背包
    Inventory,
    /// 丢弃物品
    Drop,
    /// 攻击 / 破坏
    Attack,
    /// 使用 / 放置
    Use,
    /// 切换飞行
    ToggleFly,
    /// 呼出聊天
    Chat,
    /// 呼出菜单（等价 ESC）
    Menu,
}

impl ControlAction {
    /// 该动作是否由方向键 / 摇杆驱动（决定能否出现在 `dpad` / `joystick` 控件上）。
    pub fn is_directional(self) -> bool {
        matches!(
            self,
            Self::Forward | Self::Back | Self::Left | Self::Right
        )
    }

    /// 稳定标识，用于错误文案与前端展示键。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Back => "back",
            Self::Left => "left",
            Self::Right => "right",
            Self::Jump => "jump",
            Self::Sneak => "sneak",
            Self::Sprint => "sprint",
            Self::Inventory => "inventory",
            Self::Drop => "drop",
            Self::Attack => "attack",
            Self::Use => "use",
            Self::ToggleFly => "toggle_fly",
            Self::Chat => "chat",
            Self::Menu => "menu",
        }
    }
}

/// 归一化矩形：`x`/`y` 是左上角，`w`/`h` 是宽高。
///
/// 取值 0..1，相对**屏幕短边**解释（见 [`ControlLayout::coordinate_space`] 说明）；
/// Kotlin 侧换算为像素并做边界夹取，保证控件永远留在可见区域内。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// 夹取到 0..1 并保证有最小尺寸（比例）。
    pub fn normalized(self) -> Self {
        const MIN: f32 = 0.02;
        let w = self.w.clamp(MIN, 1.0);
        let h = self.h.clamp(MIN, 1.0);
        Self {
            x: self.x.clamp(0.0, 1.0 - w),
            y: self.y.clamp(0.0, 1.0 - h),
            w,
            h,
        }
    }

    /// 是否与另一矩形重叠（用于前端提示与默认布局自检）。
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }
}

/// 一个控件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Control {
    /// 实例内唯一 id；前端增删改用它定位，不用下标（下标会因重排而错位）。
    pub id: String,
    /// 控件类型（`dpad` / `joystick` / `button` / `look`）。
    pub kind: String,
    /// 归一化位置与尺寸。
    pub rect: Rect,
    /// 显示名（i18n 键或用户文本；空则前端按 `kind` 取默认文案）。
    #[serde(default)]
    pub label: String,
    /// 绑定的动作；`look` 控件忽略此字段。
    #[serde(default)]
    pub action: Option<ControlAction>,
    /// 不透明度 0..1。
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// 是否在游戏内可见（编辑器用它做「隐藏但保留」）。
    #[serde(default = "default_true")]
    pub visible: bool,
}

fn default_opacity() -> f32 {
    0.5
}

fn default_true() -> bool {
    true
}

/// 一份完整布局。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlLayout {
    /// schema 版本（见 [`SCHEMA_VERSION`]）。
    pub version: u32,
    /// 坐标空间标识，恒为 [`COORDINATE_SPACE`]。
    pub coordinate_space: String,
    pub controls: Vec<Control>,
}

impl Default for ControlLayout {
    /// 默认布局：左手方向键 + 右手跳跃/潜行 + 全屏视角区。
    ///
    /// 刻意贴近 MCBE 习惯（左手移动、右手动作），且**不预置**攻击/使用按钮——
    /// 那两个动作在触屏上靠点击屏幕完成，放成按钮反而遮挡视线。玩家需要时在
    /// 编辑器里自己加。
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            coordinate_space: COORDINATE_SPACE.to_string(),
            controls: vec![
                Control {
                    id: "dpad".into(),
                    kind: KIND_DPAD.into(),
                    rect: Rect { x: 0.04, y: 0.55, w: 0.28, h: 0.40 },
                    label: String::new(),
                    action: None,
                    opacity: 0.45,
                    visible: true,
                },
                Control {
                    id: "jump".into(),
                    kind: KIND_BUTTON.into(),
                    rect: Rect { x: 0.845, y: 0.62, w: 0.13, h: 0.18 },
                    label: String::new(),
                    action: Some(ControlAction::Jump),
                    opacity: 0.5,
                    visible: true,
                },
                Control {
                    id: "sneak".into(),
                    kind: KIND_BUTTON.into(),
                    rect: Rect { x: 0.72, y: 0.68, w: 0.11, h: 0.15 },
                    label: String::new(),
                    action: Some(ControlAction::Sneak),
                    opacity: 0.5,
                    visible: true,
                },
                Control {
                    id: "inventory".into(),
                    kind: KIND_BUTTON.into(),
                    rect: Rect { x: 0.02, y: 0.06, w: 0.09, h: 0.13 },
                    label: String::new(),
                    action: Some(ControlAction::Inventory),
                    opacity: 0.5,
                    visible: true,
                },
                Control {
                    id: "menu".into(),
                    kind: KIND_BUTTON.into(),
                    rect: Rect { x: 0.13, y: 0.06, w: 0.08, h: 0.12 },
                    label: String::new(),
                    action: Some(ControlAction::Menu),
                    opacity: 0.5,
                    visible: true,
                },
            ],
        }
    }
}

impl ControlLayout {
    /// 校验并规整：位置夹取、id 唯一、类型与动作合法。
    ///
    /// 返回规整后的副本而不是就地改：调用方（IPC 边界）拿到的应当是**可持久化
    /// 的规范形态**，非法输入在这里一次性收敛，Kotlin 侧因此可以假定数据可靠。
    pub fn validated(mut self) -> Result<Self, KernelError> {
        if self.version > SCHEMA_VERSION {
            return Err(KernelError::InvalidArgument(format!(
                "布局版本 {} 高于本启动器支持的 {SCHEMA_VERSION}，请升级启动器后再编辑",
                self.version
            )));
        }
        self.version = SCHEMA_VERSION;
        if self.coordinate_space != COORDINATE_SPACE {
            // 不认识的坐标空间不能按比例解释，否则布局会整片飞到屏幕外。
            return Err(KernelError::InvalidArgument(format!(
                "不支持的坐标空间 `{}`（期望 `{COORDINATE_SPACE}`）",
                self.coordinate_space
            )));
        }
        if self.controls.len() > MAX_CONTROLS {
            return Err(KernelError::InvalidArgument(format!(
                "控件数量超过上限 {MAX_CONTROLS}"
            )));
        }

        let mut seen = std::collections::HashSet::new();
        for control in &mut self.controls {
            if control.id.trim().is_empty() {
                return Err(KernelError::InvalidArgument("控件缺少 id".into()));
            }
            if !seen.insert(control.id.clone()) {
                return Err(KernelError::InvalidArgument(format!(
                    "控件 id `{}` 重复",
                    control.id
                )));
            }
            match control.kind.as_str() {
                KIND_DPAD | KIND_JOYSTICK => {
                    if let Some(action) = control.action {
                        if !action.is_directional() {
                            return Err(KernelError::InvalidArgument(format!(
                                "{} 控件 `{}` 只能绑定方向动作，收到 `{}`",
                                control.kind,
                                control.id,
                                action.as_str()
                            )));
                        }
                    }
                }
                KIND_BUTTON => {
                    let action = control.action.ok_or_else(|| {
                        KernelError::InvalidArgument(format!("按钮 `{}` 必须绑定动作", control.id))
                    })?;
                    if action.is_directional() {
                        return Err(KernelError::InvalidArgument(format!(
                            "按钮 `{}` 不能绑定方向动作 `{}`，请用 dpad / joystick",
                            control.id,
                            action.as_str()
                        )));
                    }
                }
                KIND_LOOK => {
                    // 视角区不接受动作绑定：它在游戏里就是「拖动转视角」。
                    control.action = None;
                }
                other => {
                    return Err(KernelError::InvalidArgument(format!(
                        "未知控件类型 `{other}`（控件 `{}`）",
                        control.id
                    )));
                }
            }
            control.rect = control.rect.normalized();
            control.opacity = control.opacity.clamp(0.1, 1.0);
        }
        Ok(self)
    }

    /// 解析 JSON 文本（含未知版本与非法字段的报错）。
    pub fn from_json(raw: &str) -> Result<Self, KernelError> {
        let layout: Self = serde_json::from_str(raw).map_err(|e| {
            KernelError::InvalidArgument(format!("{CONTROLS_FILE} 解析失败: {e}"))
        })?;
        layout.validated()
    }

    /// 序列化为落盘文本（缩进便于人工排查）。
    pub fn to_json(&self) -> Result<String, KernelError> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// 控件数量上限。
///
/// 不是性能考量（几十个 View 无所谓），而是**可用性防线**：满屏按钮等于不可玩，
/// 且极端数量会让布局文件变成不可维护的垃圾桶。
pub const MAX_CONTROLS: usize = 48;

// ---------------------------------------------------------------- 读写与上下文

/// 读取布局。
///
/// 文件不存在时返回 [`ControlLayout::default`] 并**不落盘**：默认布局是「还没
/// 配置过」的表达，不是一个需要立即固化的用户选择——固化会让后续调整默认值
/// 对老用户失效。用户首次保存时才会写文件。
pub fn load_at(version_dir: &Path) -> Result<ControlLayout, KernelError> {
    let path = version_dir.join(CONTROLS_FILE);
    if !path.is_file() {
        return Ok(ControlLayout::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        // 空文件（例如上次写入被系统杀死截断）：退回默认而不是报错，玩家还能玩。
        return Ok(ControlLayout::default());
    }
    ControlLayout::from_json(&raw)
}

/// 原子写入布局。
///
/// 「写临时文件 + rename」：安卓上游戏进程随时可能被系统杀掉，就地覆盖会留下
/// 半截 JSON；而 `load_at` 对半截文件的容错是「退回默认布局」——那意味着玩家的
/// 自定义布局无声消失。原子替换让「旧内容」与「新内容」之间没有中间态。
pub fn save_at(version_dir: &Path, layout: &ControlLayout) -> Result<(), KernelError> {
    let layout = layout.clone().validated()?;
    let path = version_dir.join(CONTROLS_FILE);
    let temp = version_dir.join(format!("{CONTROLS_FILE}.tmp"));
    let body = layout.to_json()?;
    std::fs::write(&temp, body.as_bytes())?;
    std::fs::rename(&temp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        KernelError::Io(std::io::Error::new(
            e.kind(),
            format!("写入 {CONTROLS_FILE} 失败: {e}"),
        ))
    })?;
    Ok(())
}

/// 解析实例目录（拒绝路径逃逸；与其它实例文件同一入口）。
fn instance_dir(kernel: &KernelContext, name: &str) -> Result<PathBuf, KernelError> {
    let dir = resolve_version_dir(&kernel.versions_root(), name)?;
    if !dir.is_dir() {
        return Err(KernelError::InvalidArgument(format!("实例 `{name}` 不存在")));
    }
    Ok(dir)
}

/// 读取某实例的触控布局。
pub fn load(kernel: &KernelContext, name: &str) -> Result<ControlLayout, KernelError> {
    load_at(&instance_dir(kernel, name)?)
}

/// 保存某实例的触控布局（原子写）。
pub fn save(
    kernel: &KernelContext,
    name: &str,
    layout: &ControlLayout,
) -> Result<ControlLayout, KernelError> {
    let dir = instance_dir(kernel, name)?;
    save_at(&dir, layout)?;
    // 事件让同实例的其它视图（开始页、设置页）同步，不必各自轮询文件。
    kernel.events().publish(
        "instance.controls.changed",
        serde_json::json!({ "name": name }),
    );
    Ok(layout.clone().validated()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "copper_controls_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 缺文件必须给默认布局，且**不得**顺手写盘：默认值后续可能调整，
    /// 提前固化会让老用户永远拿不到新默认。
    #[test]
    fn missing_file_yields_default_without_writing() {
        let dir = temp_dir("missing");
        let layout = load_at(&dir).unwrap();
        assert_eq!(layout.version, SCHEMA_VERSION);
        assert!(!layout.controls.is_empty());
        assert!(
            !dir.join(CONTROLS_FILE).exists(),
            "读取默认布局不应落盘"
        );
    }

    /// 空文件 / 截断文件退回默认布局，而不是让玩家因为一次系统杀进程就进不去游戏。
    #[test]
    fn truncated_file_falls_back_to_default() {
        let dir = temp_dir("truncated");
        std::fs::write(dir.join(CONTROLS_FILE), b"").unwrap();
        assert_eq!(load_at(&dir).unwrap(), ControlLayout::default());

        std::fs::write(dir.join(CONTROLS_FILE), b"{ \"version\": 1, \"cont").unwrap();
        assert!(
            load_at(&dir).is_err(),
            "非空但语法坏掉的 JSON 必须报错——静默重置会无声吞掉用户的布局"
        );
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = temp_dir("roundtrip");
        let mut layout = ControlLayout::default();
        layout.controls[0].rect = Rect { x: 0.2, y: 0.3, w: 0.25, h: 0.35 };
        layout.controls[0].opacity = 0.7;
        save_at(&dir, &layout).unwrap();

        let loaded = load_at(&dir).unwrap();
        assert_eq!(loaded.controls[0].rect.x, 0.2);
        assert_eq!(loaded.controls[0].opacity, 0.7);
        assert!(
            !dir.join(format!("{CONTROLS_FILE}.tmp")).exists(),
            "临时文件必须已被 rename 消耗掉"
        );
    }

    /// 比例坐标必须夹取，否则前端一次误拖就把控件丢到屏幕外，玩家再也抓不回来。
    #[test]
    fn rect_is_clamped_into_screen() {
        let rect = Rect { x: 5.0, y: -3.0, w: 3.0, h: 0.0 }.normalized();
        assert!(rect.w <= 1.0 && rect.h >= 0.02);
        assert_eq!(rect.y, 0.0);
        assert!(rect.x <= 1.0 - rect.w + f32::EPSILON);
    }

    /// 类型与动作必须匹配：把跳跃绑到摇杆、把方向绑到按钮都是布局错误，
    /// 静默接受会让玩家得到一个「按了没反应」的控件却查不出原因。
    #[test]
    fn kind_and_action_are_cross_checked() {
        let jump_on_stick = ControlLayout {
            controls: vec![Control {
                id: "s".into(),
                kind: KIND_JOYSTICK.into(),
                rect: Rect { x: 0.0, y: 0.0, w: 0.2, h: 0.2 },
                label: String::new(),
                action: Some(ControlAction::Jump),
                opacity: 0.5,
                visible: true,
            }],
            ..ControlLayout::default()
        };
        assert!(jump_on_stick.validated().is_err());

        let forward_on_button = ControlLayout {
            controls: vec![Control {
                id: "b".into(),
                kind: KIND_BUTTON.into(),
                rect: Rect { x: 0.0, y: 0.0, w: 0.2, h: 0.2 },
                label: String::new(),
                action: Some(ControlAction::Forward),
                opacity: 0.5,
                visible: true,
            }],
            ..ControlLayout::default()
        };
        assert!(forward_on_button.validated().is_err());

        let button_without_action = ControlLayout {
            controls: vec![Control {
                id: "b".into(),
                kind: KIND_BUTTON.into(),
                rect: Rect { x: 0.0, y: 0.0, w: 0.2, h: 0.2 },
                label: String::new(),
                action: None,
                opacity: 0.5,
                visible: true,
            }],
            ..ControlLayout::default()
        };
        assert!(button_without_action.validated().is_err());
    }

    #[test]
    fn duplicate_ids_and_unknown_kinds_are_rejected() {
        let dup = ControlLayout {
            controls: vec![
                Control {
                    id: "a".into(),
                    kind: KIND_LOOK.into(),
                    rect: Rect { x: 0.0, y: 0.0, w: 0.2, h: 0.2 },
                    label: String::new(),
                    action: None,
                    opacity: 0.5,
                    visible: true,
                },
                Control {
                    id: "a".into(),
                    kind: KIND_LOOK.into(),
                    rect: Rect { x: 0.3, y: 0.0, w: 0.2, h: 0.2 },
                    label: String::new(),
                    action: None,
                    opacity: 0.5,
                    visible: true,
                },
            ],
            ..ControlLayout::default()
        };
        assert!(dup.validated().is_err());

        let unknown = ControlLayout {
            coordinate_space: COORDINATE_SPACE.into(),
            controls: vec![Control {
                id: "x".into(),
                kind: "wiimote".into(),
                rect: Rect { x: 0.0, y: 0.0, w: 0.2, h: 0.2 },
                label: String::new(),
                action: None,
                opacity: 0.5,
                visible: true,
            }],
            ..ControlLayout::default()
        };
        assert!(unknown.validated().is_err());
    }

    /// 更高 schema 版本的布局必须拒绝：新版本可能带来本版本不认识的语义，
    /// 按旧理解渲染等于静默走样。
    #[test]
    fn future_schema_version_is_rejected() {
        let raw = format!(
            r#"{{"version": {}, "coordinateSpace": "{COORDINATE_SPACE}", "controls": []}}"#,
            SCHEMA_VERSION + 1
        );
        assert!(ControlLayout::from_json(&raw).is_err());
    }

    /// 未知坐标空间必须拒绝：按比例解释像素坐标会让整个布局飞出屏幕。
    #[test]
    fn unknown_coordinate_space_is_rejected() {
        let raw = r#"{"version": 1, "coordinateSpace": "pixel", "controls": []}"#;
        assert!(ControlLayout::from_json(raw).is_err());
    }

    /// 视角区不接受动作绑定：它在游戏里就是「拖动转视角」，绑一个动作只会误导。
    #[test]
    fn look_control_drops_action() {
        let raw = r#"{
            "version": 1,
            "coordinateSpace": "fraction",
            "controls": [
                {"id": "look", "kind": "look", "rect": {"x":0,"y":0,"w":1,"h":1},
                 "action": "jump", "opacity": 0.5, "visible": true}
            ]
        }"#;
        let layout = ControlLayout::from_json(raw).unwrap();
        assert_eq!(layout.controls[0].action, None);
        assert!(!layout.controls[0].rect.overlaps(&Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }));
    }

    /// 契约测试：JSON 字段名必须与前端 / Kotlin 侧的 camelCase 读法一致。
    /// 这是跨语言契约，字段名漂移会让整份布局读成默认值（静默失效）。
    #[test]
    fn json_field_names_are_the_cross_language_contract() {
        let raw = ControlLayout::default().to_json().unwrap();
        for field in ["\"version\"", "\"coordinateSpace\"", "\"controls\"", "\"rect\"", "\"kind\"", "\"opacity\"", "\"visible\"", "\"action\"", "\"label\"", "\"id\""] {
            assert!(raw.contains(field), "布局 JSON 缺少契约字段 {field}:\n{raw}");
        }
        // 动作是 snake_case（与 Rust 枚举序列化一致），不是 camelCase。
        assert!(raw.contains("\"jump\"") || raw.contains("\"sneak\""));
    }
}
