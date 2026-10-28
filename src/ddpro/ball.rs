//! 主控端专业窗口的悬浮球和菜单（P1 第 2 项「专业会话只有一个窗口」）：专业会话期间通用窗口隐藏，
//! 操作入口都在这里。外观和手感沿用旧 DreamDesk 用户已经习惯的（42 像素圆角方块、默认缩在右上角顶边、可拖动贴边、
//! 点开菜单），改掉审查发现的问题（P1 实施记录第 2 节）：动作在松开时才执行；鼠标在球 / 菜单上时移动、按键、
//! 滚轮都不发给远程；Esc 关菜单；只在状态变化时重画、只画球和菜单那么大。
//!
//! 球的尺寸、投影、字体照旧 DreamDesk 的 `client/src/dd_ui_ball.c`（用户 2026-10-20 要求）；菜单的分组、顺序、叫法、图标、
//! 选中样式 10-27 改成界面原型（dreamdesk/design/prototype/ProBall.html），和通用窗口的控制中心一致，最多三级，图标都是图标库原图（`ball_icons.rs`）。
//! 右下角的网络状态块（菜单开关，默认隐藏）显示的是 RustDesk 质量面板在专业模式下的同一份实测值。
//!
//! 本模块只管布局、绘制和交互；画出来的图交给专业窗口的叠加图层（`ddpro::view::Window::set_layer`）。
//! 画法照 RustDesk 白板功能（`src/whiteboard`）：tiny-skia 画形状，ttf-parser 取字形轮廓。
//! 按窗口坐标（逻辑像素）画：屏幕缩放不是 100% 时由合成器放大（会发虚），专业画面那时本来也不逐像素（质量面板会标「Scaled」）。

#[path = "ball_icons.rs"]
mod icons;

use ddpro::{view::LayerImage, view_toggle, ViewMenu, ViewTool};
use hbb_common::log;
use std::time::{Duration, Instant};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Point, Rect, Stroke, Transform};
use ttf_parser::Face;

/// 叠加图层的编号（远程光标 3 由 view.rs 画）
const LAYER_BALL: usize = 0;
const LAYER_MENU: usize = 1;
const LAYER_STATUS: usize = 2;
const LAYER_TOAST: usize = 4;

const BALL: f32 = 42.0;
const BALL_RADIUS: f32 = 13.0;
const BORDER: f32 = 4.0;
/// 缩进的球「原来露边」的宽度：缩进时整块撤掉（见 [`Ball::render`]），这个宽度只用来定鼠标靠近多少就滑出来
const PEEK: f32 = 15.0;
/// 默认位置离右边的距离：躲开被控端最大化窗口右上角的最小化 / 最大化 / 关闭按钮（旧实现的取值）
const RIGHT_GAP: f32 = 220.0;
/// 松手时球心离边多近就贴边
const SNAP: f32 = 42.0;
/// 按下后移动超过这么多才算拖动，否则算点击
const DRAG_SLOP: f32 = 3.0;
/// 鼠标离缩进的球多近就滑出来
const REVEAL: f32 = 24.0;
/// 停在屏幕中间、没人碰多久就回到顶边
const IDLE_DOCK: Duration = Duration::from_secs(1);
/// 指针锁定后屏幕正中的提示显示多久
const TOAST_FOR: Duration = Duration::from_secs(3);
// 面板尺寸、配色、字号照界面原型（dreamdesk/design/prototype/ProBall.html，和通用窗口的控制中心菜单同一套：
// 宽 230、行高 36、行左右各缩进 6 的圆角悬停块、图标 18、字 14；单选是文字前一个 5 像素黑点，多选是左边一个对勾）
const MENU_W: f32 = 230.0;
const SUB_W: f32 = 200.0;
/// 行的悬停块离面板左右边的距离，和行里内容的左右内边距
const ROW_INSET: f32 = 6.0;
const ROW_PAD: f32 = 10.0;
/// 面板里一个图标都没有时文字的起点；有可选中的行时所有行的文字都往右让出黑点 / 对勾的位置（原型 .dd-mi--on：行内左边距 26）
const SUB_TEXT_X: f32 = ROW_INSET + ROW_PAD;
const SUB_DOT_TEXT_X: f32 = ROW_INSET + 26.0;
/// 单选项选中的黑点：直径 5，圆心在行内 12 + 半径处
const DOT: f32 = 5.0;
const SUB_DOT_X: f32 = ROW_INSET + 12.0 + DOT / 2.0;
/// 多选（开关）项选中的对勾（Lucide check，原型 .dd-mi--check：14 像素）
const CHECK: f32 = 14.0;
const ROW_H: f32 = 36.0;
/// 分组之间的分隔线占的高度（线在正中）
const DIVIDER_H: f32 = 9.0;
/// 小标题（原型 .dd-menu__head：上 6 下 4、12 号字）和说明文字（.dd-note：行高 18、下 6）；两者文字都从行内 16 起
const HEAD_H: f32 = 26.0;
const NOTE_LINE_H: f32 = 18.0;
const NOTE_PAD_B: f32 = 6.0;
const NOTE_X: f32 = 16.0;
const SMALL_PX: f32 = 12.0;
/// 面板上下内边距（原型 .dd-menu：padding 6px 0）；行的左右外边距是 [`ROW_INSET`]，悬停块圆角 [`ITEM_RADIUS`]（.dd-mi：radius-md）。
/// 面板里一行的位置只有 [`item_rect`] 一处定义，排版（`layout`）、绘制（`draw_panel`）、命中判断（`node_at`）都用它
const PAD: f32 = 6.0;
const ITEM_RADIUS: f32 = 6.0;
const PANEL_RADIUS: f32 = 8.0;
/// 投影向外扩散的像素（球和面板都有）
const SHADOW: f32 = 10.0;
const GAP: f32 = 8.0;
/// 子面板和父面板之间的缝
const PANEL_GAP: f32 = 4.0;
const ICON: f32 = 18.0;
/// 图标中心、文字左起距面板左边
const ICON_X: f32 = ROW_INSET + ROW_PAD + ICON / 2.0;
const TEXT_X: f32 = ROW_INSET + ROW_PAD + ICON + 10.0;
/// 右边的子菜单箭头（16 像素）中心离面板右边
const MARK_X: f32 = ROW_INSET + ROW_PAD + 8.0;
const FONT_PX: f32 = 14.0;

// 配色和通用窗口一致（flutter/lib/desktop/widgets/dd_theme.dart）
const C_PANEL: [u8; 4] = [255, 255, 255, 255];
const C_TEXT: [u8; 4] = [31, 35, 41, 255];
const C_TEXT2: [u8; 4] = [100, 106, 115, 255];
const C_DIM: [u8; 4] = [154, 160, 168, 255];
const C_HOVER: [u8; 4] = [0, 0, 0, 15];
const C_LINE: [u8; 4] = [220, 223, 228, 255];
const C_DANGER: [u8; 4] = [209, 52, 56, 255];
/// 球：常态浅灰底 + 浅灰带状边框；悬停时底转白、边框叠蓝。图标颜色始终不变
const C_BALL: [u8; 4] = [241, 241, 242, 247];
const C_BALL_H: [u8; 4] = [255, 255, 255, 252];
const C_BAND: [u8; 4] = [227, 227, 230, 255];
const C_BAND_H: [u8; 4] = [177, 203, 255, 255];
const C_BALL_FG: [u8; 4] = [71, 77, 87, 255];
/// 网络状态块（照 UU 的「网络状态」，原型 RemoteStats.html ③）：12 号等宽白字、行高 16、左右留 12 上下留 8、4 圆角、黑底 70%；
/// 一行一个数值、不写名称；离屏幕底边 110 像素（让开被控端任务栏）
const STATUS_PX: f32 = 12.0;
const STATUS_LINE_H: f32 = 16.0;
const STATUS_PAD_X: f32 = 12.0;
const STATUS_PAD_Y: f32 = 8.0;
const STATUS_BOTTOM: f32 = 110.0;
const C_STATUS_BG: [u8; 4] = [0, 0, 0, 179];
const C_STATUS_FG: [u8; 4] = [255, 255, 255, 255];
/// 指针锁定提示（原型 .dd-toast）：高 38、左右内边距 16、6 圆角、13 号字
const TOAST_H: f32 = 38.0;
const TOAST_PAD_X: f32 = 16.0;
const TOAST_PX: f32 = 13.0;
const C_TOAST_BG: [u8; 4] = [0x1f, 0x23, 0x29, 0xe6];

/// Linux evdev 码
const BTN_LEFT: u32 = 0x110;
const KEY_ESC: u32 = 1;

/// 菜单里选中的动作，由专业窗口交给会话进程（「断开连接」由专业窗口自己按用户关窗结束）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SwitchDisplay(i32),
    /// 把远程这块屏改成这个分辨率（宽, 高）
    ChangeResolution(u32, u32),
    /// 网络状态块开 / 关之后的状态（会话进程记下来，换屏开新窗口时照旧）
    ShowStatus(bool),
    /// 麦克风开 / 关之后的状态（会话进程发起 / 结束语音通话，并记下来给换屏开的新窗口）
    Mic(bool),
    /// HDR 要开 / 关（整个会话）：会话进程请被控端切、再切本机，专业窗口随之重开，勾不勾看重开后谈成的格式
    Hdr(bool),
    GeneralMode,
    CtrlAltDel,
    LockScreen,
    Disconnect,
    /// 会话选项里的开关（静音、断开后锁定、禁止对方本地操作、剪贴板、文件粘贴、反转滚轮、交换左右键、铺满所有显示器）开 / 关之后的值
    Toggle(view_toggle::Kind, bool),
    /// 鼠标模式：true = 远程鼠标
    MouseRemote(bool),
    /// 开文件传输 / 终端窗口
    OpenTool(ViewTool),
    /// 重启远程电脑（菜单里已经点过确认）
    RestartRemote,
}

/// 菜单的初值（来自 ViewStart）
pub struct MenuInit {
    /// 这块远程屏的型号，「显示屏 1（VX2723）」括号里的；空 = 不写
    pub label: String,
    pub menu: ViewMenu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Top,
    Left,
    Right,
}

/// 菜单面板：`None` 是一级，其余是从某一行展开的子菜单
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubId {
    Monitor,
    Resolutions,
    Sound,
    Security,
    Peripherals,
    Tools,
    More,
    Restart,
}

/// 点了会翻转状态的开关
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Status,
    Mic,
    Hdr,
    Toggle(view_toggle::Kind),
}

/// 点了一行之后做什么
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Go {
    /// 展开子菜单（随悬停展开，点它不用再做什么）
    Sub(SubId),
    /// 什么也不做、菜单保持
    Nothing,
    /// 只关菜单（点了已经选中的单选项）
    Close,
    /// 发动作、关菜单
    Send(Action),
    /// 翻转开关、发动作；多选项菜单保持展开
    Flip(Flag),
    /// 选本地 / 远程鼠标
    Mouse(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    None,
    /// 单选：文字前一个黑点
    Dot,
    /// 多选：左边一个对勾
    Check,
}

#[derive(Clone)]
struct Entry {
    icon: Option<&'static icons::Icon>,
    label: String,
    /// 右边的灰字（说明为什么灰着）
    hint: Option<String>,
    mark: Mark,
    on: bool,
    danger: bool,
    /// 灰掉、不可点
    off: bool,
    go: Go,
}

impl Entry {
    fn new(label: String, go: Go) -> Self {
        Self {
            icon: None,
            label,
            hint: None,
            mark: Mark::None,
            on: false,
            danger: false,
            off: false,
            go,
        }
    }

    fn icon(mut self, icon: &'static icons::Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    fn dot(mut self, on: bool) -> Self {
        self.mark = Mark::Dot;
        self.on = on;
        self
    }

    fn check(mut self, on: bool) -> Self {
        self.mark = Mark::Check;
        self.on = on;
        self
    }

    fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    fn disabled(mut self, off: bool) -> Self {
        self.off = off;
        self
    }

    fn hint(mut self, hint: String) -> Self {
        self.hint = Some(hint);
        self
    }
}

enum Node {
    Entry(Entry),
    Head(String),
    Note(String),
    Divider,
}

/// 面板里一个节点的位置：上沿离面板上沿多远、高多少；说明文字折好的行
struct Row {
    y: f32,
    h: f32,
    lines: Vec<String>,
}

struct Panel {
    id: Option<SubId>,
    nodes: Vec<Node>,
    rows: Vec<Row>,
    rect: (f32, f32, f32, f32),
    /// 子面板放在父面板右边还是左边：它的子面板优先接着往同一侧放
    right: bool,
    /// 这个面板里文字的起点、选中标记（黑点 / 对勾）的圆心，距面板左边
    text_x: f32,
    mark_cx: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Ball,
    /// （哪个面板，第几个节点）：按下后悬停可能把子菜单换成别的，松开时要还是同一个面板的同一行才算
    Node(Option<SubId>, usize),
    /// 菜单开着时点在外面：关菜单，这一下不发给远程
    Outside,
    /// 点在菜单面板上、但不在任何一行上（分隔线、小标题、说明、上下留白）：什么也不做
    Panel,
}

struct Press {
    target: Target,
    at: (f32, f32),
    ball_at: (f32, f32),
    dragging: bool,
}

pub struct Ball {
    screen: (f32, f32),
    face: Option<Face<'static>>,
    /// 网络状态块用的等宽字体（没有就用菜单字体）
    mono: Option<Face<'static>>,
    show_status: bool,
    /// 麦克风开着（语音通话进行中）
    mic: bool,
    /// 这个专业窗口谈成的是 HDR 画面（「HDR」打勾）；能不能点（两头的屏都支持 HDR）
    hdr: bool,
    hdr_available: bool,
    status_lines: Vec<String>,
    status_dirty: bool,
    /// 球完全露出时的左上角
    x: f32,
    y: f32,
    edge: Option<Edge>,
    tucked: bool,
    hover: bool,
    menu_open: bool,
    /// 打开时的快照，只读出现条件（has_*）；开关的现值在 `toggled`
    menu: ViewMenu,
    /// 这块远程屏的型号
    label: String,
    /// 现在开着的会话选项开关
    toggled: Vec<view_toggle::Kind>,
    /// 鼠标模式：现在是不是远程鼠标；非空 = 远程鼠标用不了，这是原因
    mouse_remote: bool,
    mouse_unavailable: String,
    /// 一级 + 当前展开的子菜单（最多三级），状态变了就整个重新生成
    panels: Vec<Panel>,
    /// 展开的子菜单链：`open[k]` 是从第 k 个面板的某一行展开的
    open: Vec<SubId>,
    hover_node: Option<(usize, usize)>,
    press: Option<Press>,
    /// 按下时落在球 / 菜单上（被拦下）的按键：松开也拦下
    swallowed: Vec<u32>,
    esc_swallowed: bool,
    pointer: (f32, f32),
    free_since: Option<Instant>,
    display_count: u32,
    display_index: i32,
    /// 多屏时这个窗口固定看这块屏（P1 第 8 项）：「切换下一个屏幕」灰掉，别的屏归别的窗口
    display_fixed: bool,
    /// 分辨率：远程这块屏现在的分辨率、支持的分辨率、原始分辨率（被控端报来的，和 RustDesk 工具栏用的同一份）
    remote_size: (u32, u32),
    resolutions: Vec<(u32, u32)>,
    original: (u32, u32),
    /// 指针被锁在窗口里（远程鼠标）：球和菜单都不出现，也不处理任何输入
    locked: bool,
    toast_until: Option<Instant>,
    toast_dirty: bool,
    dirty: bool,
}

impl Ball {
    pub fn new(
        (w, h): (u32, u32),
        display_count: u32,
        display_index: i32,
        display_fixed: bool,
        show_status: bool,
        mic: bool,
        hdr: bool,
        hdr_available: bool,
        init: MenuInit,
    ) -> Self {
        let (face, mono) = load_faces();
        if face.is_none() {
            log::warn!("ddpro: pro view menu has no font, showing no text");
        }
        let screen = (w as f32, h as f32);
        let MenuInit { label, menu } = init;
        let toggled = [
            (view_toggle::Kind::MUTE, menu.mute),
            (view_toggle::Kind::LOCK_AFTER_END, menu.lock_after_end),
            (view_toggle::Kind::BLOCK_INPUT, menu.block_input),
            (view_toggle::Kind::CLIPBOARD, menu.clipboard),
            (view_toggle::Kind::FILE_PASTE, menu.file_paste),
            (view_toggle::Kind::REVERSE_WHEEL, menu.reverse_wheel),
            (view_toggle::Kind::SWAP_BUTTONS, menu.swap_buttons),
            (view_toggle::Kind::ALL_DISPLAYS, menu.all_displays),
        ]
        .into_iter()
        .filter_map(|(k, on)| on.then_some(k))
        .collect();
        Self {
            screen,
            face,
            mono,
            show_status,
            mic,
            hdr,
            hdr_available,
            status_lines: Vec::new(),
            status_dirty: show_status,
            x: (screen.0 - RIGHT_GAP - BALL).max(0.0),
            y: 0.0,
            edge: Some(Edge::Top),
            tucked: true,
            hover: false,
            menu_open: false,
            mouse_remote: menu.mouse_remote,
            mouse_unavailable: menu.mouse_unavailable.clone(),
            menu,
            label,
            toggled,
            panels: Vec::new(),
            open: Vec::new(),
            hover_node: None,
            press: None,
            swallowed: Vec::new(),
            esc_swallowed: false,
            pointer: (-1.0, -1.0),
            free_since: None,
            display_count: display_count.max(1),
            display_index,
            display_fixed,
            remote_size: (0, 0),
            resolutions: Vec::new(),
            original: (0, 0),
            locked: false,
            toast_until: None,
            toast_dirty: false,
            dirty: true,
        }
    }

    /// 被控端报来远程这块屏的分辨率信息（列表为空时只更新当前分辨率，菜单沿用上次的列表）
    pub fn set_resolutions(&mut self, current: (u32, u32), mut resolutions: Vec<(u32, u32)>, original: (u32, u32)) {
        self.remote_size = current;
        if !resolutions.is_empty() {
            // 和 RustDesk 一样排：宽从大到小、宽相同按高从大到小（`model.dart` 解析分辨率列表处）
            resolutions.sort_by(|a, b| b.cmp(a));
            self.resolutions = resolutions;
            self.original = original;
        }
        self.refresh_menu();
    }

    /// 会话进程回报鼠标模式：`remote` = 现在该不该是远程鼠标；`unavailable` 非空 = 远程鼠标用不了，这是原因
    pub fn set_mouse_mode(&mut self, remote: bool, unavailable: String) {
        self.mouse_remote = remote;
        self.mouse_unavailable = unavailable;
        self.refresh_menu();
    }

    /// 指针被锁在窗口里（远程鼠标）：关菜单、藏球、不再处理输入，屏幕正中提示怎么退出；放开后球回来
    pub fn set_pointer_locked(&mut self, locked: bool) {
        if locked == self.locked {
            return;
        }
        self.locked = locked;
        if locked {
            if self.press.as_ref().is_some_and(|p| p.dragging) {
                self.drop_ball();
            }
            self.press = None;
            self.swallowed.clear();
            self.esc_swallowed = false;
            self.hover = false;
            self.close_menu();
            self.toast_until = Some(Instant::now() + TOAST_FOR);
        } else {
            self.toast_until = None;
            self.update_hover();
        }
        self.dirty = true;
        self.toast_dirty = true;
    }

    pub fn pointer_locked(&self) -> bool {
        self.locked
    }

    /// 本机屏幕分辨率变了：球回到默认位置
    pub fn set_screen(&mut self, (w, h): (u32, u32)) {
        self.screen = (w as f32, h as f32);
        self.x = (self.screen.0 - RIGHT_GAP - BALL).max(0.0);
        self.y = 0.0;
        self.edge = Some(Edge::Top);
        self.tucked = true;
        self.close_menu();
        self.dirty = true;
        // 状态块贴在右下角，屏幕尺寸变了要按新尺寸重画
        self.status_dirty |= self.show_status;
        self.toast_dirty |= self.toast_until.is_some();
    }

    /// 鼠标移动；返回 true 表示这一下归悬浮球，不发给远程
    pub fn pointer_motion(&mut self, x: f64, y: f64) -> bool {
        if self.locked {
            return false;
        }
        self.pointer = (x as f32, y as f32);
        let mut started_drag = false;
        if let Some(p) = self.press.as_mut() {
            if p.target == Target::Ball {
                let (dx, dy) = (self.pointer.0 - p.at.0, self.pointer.1 - p.at.1);
                if !p.dragging && (dx.abs() > DRAG_SLOP || dy.abs() > DRAG_SLOP) {
                    p.dragging = true;
                    started_drag = true;
                }
                if p.dragging {
                    self.x = (p.ball_at.0 + dx).clamp(0.0, (self.screen.0 - BALL).max(0.0));
                    self.y = (p.ball_at.1 + dy).clamp(0.0, (self.screen.1 - BALL).max(0.0));
                    self.edge = None;
                    self.tucked = false;
                    self.free_since = None;
                    self.dirty = true;
                }
            }
        }
        if started_drag {
            self.close_menu();
        }
        self.update_hover();
        self.press.is_some() || self.over_ui()
    }

    /// 鼠标按键；返回（是否归悬浮球，选中的动作）
    pub fn pointer_button(&mut self, button: u32, down: bool, x: f64, y: f64) -> (bool, Option<Action>) {
        if self.locked {
            return (false, None);
        }
        self.pointer = (x as f32, y as f32);
        if down {
            let target = if self.hit_ball() {
                Target::Ball
            } else if let Some((k, i)) = self.node_at() {
                Target::Node(self.panels[k].id, i)
            } else if self.menu_open && self.over_ui() {
                // 面板上不是任何一行的地方（分隔线、小标题、说明、上下留白）：不关菜单、也不发给远程
                Target::Panel
            } else if self.menu_open {
                Target::Outside
            } else {
                return (false, None);
            };
            self.swallowed.push(button);
            if target == Target::Outside {
                self.close_menu();
            }
            if button == BTN_LEFT {
                self.press = Some(Press {
                    target,
                    at: self.pointer,
                    ball_at: self.ball_pos(),
                    dragging: false,
                });
            }
            self.free_since = None;
            self.update_hover();
            return (true, None);
        }
        let Some(i) = self.swallowed.iter().position(|&b| b == button) else {
            return (false, None);
        };
        self.swallowed.swap_remove(i);
        if button != BTN_LEFT {
            return (true, None);
        }
        let Some(p) = self.press.take() else {
            return (true, None);
        };
        let action = match p.target {
            Target::Ball if p.dragging => {
                self.drop_ball();
                None
            }
            Target::Ball => {
                if self.menu_open {
                    self.close_menu();
                } else {
                    self.open_menu();
                }
                None
            }
            // 松开时还在按下的那一行上才算选中（按下后挪开就是取消）
            Target::Node(id, i) => {
                let hit = self.node_at().filter(|&(k, j)| self.panels[k].id == id && j == i);
                match hit.map(|(k, j)| &self.panels[k].nodes[j]) {
                    Some(Node::Entry(e)) if !e.off => {
                        let go = e.go;
                        self.click(go)
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        self.update_hover();
        self.dirty = true;
        (true, action)
    }

    /// 指针离开窗口或图层。按着球 / 菜单时离开，说明指针被合成器抢走了（比如拖球时弹出系统对话框），那次松开不会来：
    /// 清掉按下状态，不然之后发给远程的鼠标移动会一直被球拦着，直到再点一下球（2026-10-20 审查第 3 条）。
    /// 没按着时从球挪到画面上也会离开图层，这时什么都不用做
    pub fn pointer_leave(&mut self) {
        if let Some(p) = self.press.take() {
            if p.dragging {
                self.drop_ball();
            }
            self.dirty = true;
        }
        self.swallowed.clear();
        // 先当指针在屏外：挪到本窗口别处时紧接着有进入 / 移动把位置更新回来；真离开窗口（划到另一块屏）就不会再有，
        // 不复位的话球一直露着、图层不撤，合成器不能直接送显示
        self.pointer = (-1.0e4, -1.0e4);
        self.update_hover();
    }

    /// 滚轮：鼠标在球 / 菜单上时不发给远程
    pub fn wheel(&self) -> bool {
        !self.locked && self.over_ui()
    }

    /// 窗口失去键盘焦点：被吞的那一下 Esc 的松开不会来了，清掉标记，不然下一次 Esc 的按下和松开都会被吞
    pub fn keyboard_leave(&mut self) {
        self.esc_swallowed = false;
    }

    /// 键盘：菜单开着时 Esc 关菜单（按下和松开都不发给远程）
    pub fn key(&mut self, evdev: u32, down: bool) -> bool {
        if self.locked || evdev != KEY_ESC {
            return false;
        }
        if down && self.menu_open {
            self.close_menu();
            self.esc_swallowed = true;
            return true;
        }
        // 按住不放时窗口自己补的连发按下：属于被吞的这一下，一并吞掉，否则远程收到一串按下、松开却被吞，Esc 卡在按下
        if down && self.esc_swallowed {
            return true;
        }
        if !down && self.esc_swallowed {
            self.esc_swallowed = false;
            return true;
        }
        false
    }

    /// 定时检查：锁定提示到时撤掉；停在屏幕中间、没人碰一会儿就回到顶边
    pub fn tick(&mut self, now: Instant) {
        if self.toast_until.is_some_and(|t| now >= t) {
            self.toast_until = None;
            self.toast_dirty = true;
        }
        let idle = !self.locked && self.edge.is_none() && !self.menu_open && self.press.is_none() && !self.hover;
        if !idle {
            self.free_since = None;
            return;
        }
        let since = *self.free_since.get_or_insert(now);
        if now.duration_since(since) >= IDLE_DOCK {
            self.free_since = None;
            self.edge = Some(Edge::Top);
            self.y = 0.0;
            self.update_hover();
            self.dirty = true;
        }
    }

    /// 网络状态块这一秒的几行字（专业窗口每秒给一次）；块开着且内容变了才重画
    pub fn set_status(&mut self, lines: Vec<String>) {
        if lines != self.status_lines {
            self.status_lines = lines;
            self.status_dirty |= self.show_status;
        }
    }

    /// 状态变了才重画：返回要更新的图层（编号, 内容；None 表示撤掉那块图层）
    pub fn render(&mut self) -> Vec<(usize, Option<LayerImage>)> {
        let mut out = Vec::new();
        if std::mem::take(&mut self.dirty) {
            if self.locked {
                out.push((LAYER_BALL, None));
                out.push((LAYER_MENU, None));
            } else {
                // 缩进时整块撤掉、一点都不露（鼠标靠近原来露边的地方照样滑出来）：屏幕上有任何叠加图层，合成器就不能把画面
                // 直接送显示、每帧都要重新合成一遍（2026-10-21 瘦客户机实测：露一条时直接送显示 0%、合成器 CPU 约 22%，
                // 不露时 100%、约 14%）。用户 2026-10-21 定：缩进时完全隐藏
                if self.tucked {
                    out.push((LAYER_BALL, None));
                } else if let Some(ball) = self.draw_ball() {
                    out.push((LAYER_BALL, Some(ball)));
                }
                out.push((LAYER_MENU, if self.menu_open { self.draw_menu() } else { None }));
            }
        }
        if std::mem::take(&mut self.status_dirty) {
            out.push((LAYER_STATUS, if self.show_status { self.draw_status() } else { None }));
        }
        if std::mem::take(&mut self.toast_dirty) {
            out.push((LAYER_TOAST, if self.toast_until.is_some() { self.draw_toast() } else { None }));
        }
        out
    }

    fn open_menu(&mut self) {
        self.menu_open = true;
        self.tucked = false;
        self.open.clear();
        self.rebuild();
        self.dirty = true;
    }

    fn close_menu(&mut self) {
        if self.menu_open {
            self.dirty = true;
        }
        self.menu_open = false;
        self.open.clear();
        self.panels.clear();
        self.hover_node = None;
    }

    /// 菜单开着时，数据变了（分辨率、鼠标模式）：按新状态重新生成
    fn refresh_menu(&mut self) {
        if self.menu_open {
            self.rebuild();
            self.dirty = true;
        }
    }

    /// 点了一行：返回要交给会话进程的动作。开关项菜单保持展开（点了就切换），动作项和单选项关菜单
    fn click(&mut self, go: Go) -> Option<Action> {
        match go {
            Go::Sub(_) | Go::Nothing => None,
            Go::Close => {
                self.close_menu();
                None
            }
            Go::Send(a) => {
                self.close_menu();
                Some(a)
            }
            Go::Mouse(remote) => {
                // 先按点的改显示，会话进程回报的最终值（set_mouse_mode）会纠正
                self.mouse_remote = remote;
                self.close_menu();
                Some(Action::MouseRemote(remote))
            }
            Go::Flip(flag) => {
                let (action, close) = self.flip(flag);
                if close {
                    self.close_menu();
                } else {
                    self.rebuild();
                    self.dirty = true;
                }
                Some(action)
            }
        }
    }

    /// 翻转一个开关；第二个值：点完要不要关菜单
    fn flip(&mut self, flag: Flag) -> (Action, bool) {
        match flag {
            Flag::Status => {
                self.show_status = !self.show_status;
                self.status_dirty = true;
                (Action::ShowStatus(self.show_status), false)
            }
            Flag::Mic => {
                self.mic = !self.mic;
                (Action::Mic(self.mic), false)
            }
            // 不在这里翻转勾：切成了专业窗口会重开，新窗口按谈成的格式打勾；没切成就照旧。菜单关掉，点了才有反馈
            Flag::Hdr => (Action::Hdr(!self.hdr), true),
            Flag::Toggle(kind) => {
                let on = !self.is_on(kind);
                if on {
                    self.toggled.push(kind);
                } else {
                    self.toggled.retain(|&k| k != kind);
                }
                // 「禁止对方本地操作」是动作项（没有勾），点了就关菜单
                (Action::Toggle(kind, on), kind == view_toggle::Kind::BLOCK_INPUT)
            }
        }
    }

    fn is_on(&self, kind: view_toggle::Kind) -> bool {
        self.toggled.contains(&kind)
    }

    /// 松手：靠近上 / 左 / 右边就贴边，否则停在原地（一会儿没人碰再回顶边）
    fn drop_ball(&mut self) {
        let (cx, cy) = (self.x + BALL / 2.0, self.y + BALL / 2.0);
        self.edge = if cy <= SNAP {
            Some(Edge::Top)
        } else if cx <= SNAP {
            Some(Edge::Left)
        } else if self.screen.0 - cx <= SNAP {
            Some(Edge::Right)
        } else {
            None
        };
        match self.edge {
            Some(Edge::Top) => self.y = 0.0,
            Some(Edge::Left) => self.x = 0.0,
            Some(Edge::Right) => self.x = (self.screen.0 - BALL).max(0.0),
            None => {}
        }
    }

    /// 球此刻在哪（缩进时是「原来露边」的位置，不画，只用来判断鼠标靠近）
    fn ball_pos(&self) -> (f32, f32) {
        match (self.edge, self.tucked) {
            (Some(Edge::Top), true) => (self.x, PEEK - BALL),
            (Some(Edge::Left), true) => (PEEK - BALL, self.y),
            (Some(Edge::Right), true) => (self.screen.0 - PEEK, self.y),
            _ => (self.x, self.y),
        }
    }

    fn hit_ball(&self) -> bool {
        let (bx, by) = self.ball_pos();
        inside(self.pointer, bx - 4.0, by - 4.0, BALL + 8.0, BALL + 8.0)
    }

    /// 缩进的球附近（鼠标靠近就滑出来）：露出的那条边外扩一圈，以及球完全露出时的位置
    fn near_ball(&self) -> bool {
        let (bx, by) = self.ball_pos();
        let (fx, fy) = match self.edge {
            Some(Edge::Top) => (self.x, 0.0),
            Some(Edge::Left) => (0.0, self.y),
            Some(Edge::Right) => (self.screen.0 - BALL, self.y),
            None => (self.x, self.y),
        };
        inside(self.pointer, bx - REVEAL, by - REVEAL, BALL + 2.0 * REVEAL, BALL + 2.0 * REVEAL)
            || inside(self.pointer, fx, fy, BALL, BALL)
    }

    fn update_hover(&mut self) {
        let tucked = self.edge.is_some() && !self.menu_open && self.press.is_none() && !self.near_ball();
        if tucked != self.tucked {
            self.tucked = tucked;
            self.dirty = true;
        }
        let hover = self.hit_ball();
        // 移到带子菜单的行就展开它；移到同一面板的别的行就把它和更深的收起（移进子菜单本身不收；分隔线、说明上不动）
        if let Some((k, i)) = self.node_at() {
            let want = match &self.panels[k].nodes[i] {
                Node::Entry(Entry { go: Go::Sub(s), .. }) => Some(*s),
                _ => None,
            };
            let changed = match want {
                Some(s) => self.open.get(k) != Some(&s) || self.open.len() != k + 1,
                None => self.open.len() > k,
            };
            if changed {
                self.open.truncate(k);
                self.open.extend(want);
                self.rebuild();
                self.dirty = true;
            }
        }
        let node = self.node_at();
        if hover != self.hover || node != self.hover_node {
            self.dirty = true;
        }
        self.hover = hover;
        self.hover_node = node;
    }

    fn over_ui(&self) -> bool {
        self.hit_ball() || (self.menu_open && self.panels.iter().any(|p| inside(self.pointer, p.rect.0, p.rect.1, p.rect.2, p.rect.3)))
    }

    /// 指针下的可点行（面板下标, 节点下标）；深的面板先判。落在分隔线、小标题、说明、留白上不算
    fn node_at(&self) -> Option<(usize, usize)> {
        let (px, py) = self.pointer;
        let (k, p) = self
            .panels
            .iter()
            .enumerate()
            .rev()
            .find(|(_, p)| inside((px, py), p.rect.0, p.rect.1, p.rect.2, p.rect.3))?;
        // 命中的就是画出来的那块圆角矩形（行左右的外边距、面板上下的内边距不算）
        let i = p.rows.iter().position(|r| {
            let (x, y, w, h) = item_rect(p.rect, r);
            inside((px, py), x, y, w, h)
        })?;
        matches!(p.nodes[i], Node::Entry(_)).then_some((k, i))
    }

    fn monitor_label(&self) -> String {
        let name = screen_name(self.display_index);
        if self.label.is_empty() {
            name
        } else {
            format!("{name}{}", tr(&format!(" ({{{}}})", self.label)))
        }
    }

    /// 某个面板此刻的节点：每次都按当前状态（初值 + 点过的开关 + 分辨率数据 + 屏幕数）重新生成，不留和显示不同步的副本
    fn nodes(&self, id: Option<SubId>) -> Vec<Node> {
        let m = &self.menu;
        let toggle = |label: &str, kind: view_toggle::Kind| {
            Node::Entry(Entry::new(tr(label), Go::Flip(Flag::Toggle(kind))).check(self.is_on(kind)))
        };
        let plain = |icon: &'static icons::Icon, label: &str, go: Go| Node::Entry(Entry::new(tr(label), go).icon(icon));
        let mut v = Vec::new();
        match id {
            None => {
                // 子菜单里没东西可放时（分辨率还没报来、也没有铺满开关）只是一行文字
                let monitor = if self.nodes(Some(SubId::Monitor)).is_empty() { Go::Nothing } else { Go::Sub(SubId::Monitor) };
                v.push(Node::Entry(Entry::new(self.monitor_label(), monitor).icon(&icons::MONITOR)));
                let next = (self.display_index + 1).rem_euclid(self.display_count as i32);
                v.push(Node::Entry(
                    Entry::new(tr("Switch to next screen"), Go::Send(Action::SwitchDisplay(next)))
                        .icon(&icons::ARROW_LEFT_RIGHT)
                        .disabled(self.display_fixed || self.display_count < 2),
                ));
                v.push(Node::Divider);
                let mut hdr = Entry::new(tr("HDR"), Go::Flip(Flag::Hdr)).check(self.hdr).disabled(!self.hdr_available);
                if !self.hdr_available {
                    hdr = hdr.hint(tr("This screen does not support HDR"));
                }
                v.push(Node::Entry(hdr));
                v.push(plain(&icons::VOLUME, "Sound", Go::Sub(SubId::Sound)));
                v.push(plain(&icons::LOCK, "Security", Go::Sub(SubId::Security)));
                v.push(plain(&icons::MOUSE, "Peripherals", Go::Sub(SubId::Peripherals)));
                v.push(plain(&icons::WRENCH, "Tools", Go::Sub(SubId::Tools)));
                v.push(plain(&icons::ELLIPSIS, "More", Go::Sub(SubId::More)));
                v.push(Node::Divider);
                v.push(Node::Entry(Entry::new(tr("End remote control"), Go::Send(Action::Disconnect)).icon(&icons::LOG_OUT).danger()));
            }
            Some(SubId::Monitor) => {
                // 分辨率照 RustDesk 工具栏的「分辨率」（`remote_toolbar.dart` 的 `_ResolutionsMenu`）：对方这块屏不止一种分辨率才有；
                // 不在支持列表里的灰掉；自定义宽高只对虚拟显示器、要输入数字，悬浮球不做
                if self.resolutions.len() >= 2 {
                    let local = (self.screen.0 as u32, self.screen.1 as u32);
                    let remote = if self.original.0 > 0 { self.original } else { self.remote_size };
                    v.push(self.resolution(&format!("Match this display ({{{}}})", size_text(local)), local));
                    v.push(self.resolution(&format!("Match the remote ({{{}}})", size_text(remote)), remote));
                    v.push(Node::Entry(Entry::new(tr("Other resolutions"), Go::Sub(SubId::Resolutions))));
                }
                if m.has_all_displays {
                    if !v.is_empty() {
                        v.push(Node::Divider);
                    }
                    v.push(toggle("Fill all my displays when connecting", view_toggle::Kind::ALL_DISPLAYS));
                }
            }
            Some(SubId::Resolutions) => {
                // 整屏放不下的截掉后面的
                let fit = ((self.screen.1 - 2.0 * PAD) / ROW_H).max(1.0) as usize;
                for &r in self.resolutions.iter().take(fit) {
                    v.push(self.resolution(&size_text(r), r));
                }
            }
            Some(SubId::Sound) => {
                v.push(toggle("Mute", view_toggle::Kind::MUTE));
                v.push(Node::Entry(Entry::new(tr("Microphone"), Go::Flip(Flag::Mic)).check(self.mic)));
            }
            Some(SubId::Security) => {
                v.push(plain(&icons::LOCK, "Insert Lock", Go::Send(Action::LockScreen)));
                if m.has_block_input {
                    let label = if self.is_on(view_toggle::Kind::BLOCK_INPUT) { "Unblock user input" } else { "Block user input" };
                    v.push(plain(&icons::BAN, label, Go::Flip(Flag::Toggle(view_toggle::Kind::BLOCK_INPUT))));
                }
                v.push(Node::Divider);
                v.push(toggle("Lock after session end", view_toggle::Kind::LOCK_AFTER_END));
            }
            Some(SubId::Peripherals) => {
                let unavailable = !self.mouse_unavailable.is_empty();
                let remote = self.mouse_remote && !unavailable;
                v.push(Node::Head(tr("Mouse mode")));
                v.push(Node::Entry(Entry::new(tr("Use the controller's mouse"), Go::Mouse(false)).dot(!remote)));
                v.push(Node::Note(tr("Better pointer response; some software or games may not work")));
                v.push(Node::Entry(Entry::new(tr("Use the controlled side's mouse"), Go::Mouse(true)).dot(remote).disabled(unavailable)));
                // 能用时说明特点、怎么退出；用不了时写原因
                v.push(Node::Note(if unavailable {
                    tr(&self.mouse_unavailable)
                } else {
                    tr("Better compatibility, but the pointer display is affected by network latency; press {Ctrl+Alt} to exit")
                }));
                let mut rest = Vec::new();
                if m.has_clipboard {
                    rest.push(toggle("Enable clipboard", view_toggle::Kind::CLIPBOARD));
                }
                if m.has_file_paste {
                    rest.push(toggle("Enable file copy and paste", view_toggle::Kind::FILE_PASTE));
                }
                if m.has_mouse_options {
                    rest.push(toggle("Reverse mouse wheel", view_toggle::Kind::REVERSE_WHEEL));
                    rest.push(toggle("swap-left-right-mouse", view_toggle::Kind::SWAP_BUTTONS));
                }
                if !rest.is_empty() {
                    v.push(Node::Divider);
                    v.extend(rest);
                }
            }
            Some(SubId::Tools) => {
                v.push(plain(&icons::FOLDER_UP, "File transfer", Go::Send(Action::OpenTool(ViewTool::VIEW_TOOL_FILE_TRANSFER))));
                v.push(plain(&icons::TERMINAL, "Terminal", Go::Send(Action::OpenTool(ViewTool::VIEW_TOOL_TERMINAL))));
            }
            Some(SubId::More) => {
                v.push(plain(&icons::KEYBOARD, "Insert Ctrl + Alt + Del", Go::Send(Action::CtrlAltDel)));
                if m.has_restart {
                    v.push(plain(&icons::POWER, "Restart remote device", Go::Sub(SubId::Restart)));
                }
                v.push(Node::Divider);
                v.push(Node::Entry(Entry::new(tr("Show quality monitor"), Go::Flip(Flag::Status)).check(self.show_status)));
                v.push(Node::Divider);
                v.push(plain(&icons::MONITOR, "Switch to general mode", Go::Send(Action::GeneralMode)));
            }
            // 通用模式点了弹确认框；专业模式下通用窗口藏着，用第三级菜单代替
            Some(SubId::Restart) => {
                v.push(Node::Entry(Entry::new(tr("Confirm restart remote device"), Go::Send(Action::RestartRemote)).danger()));
            }
        }
        v
    }

    /// 分辨率单选项：当前的那一项带点（点了只关菜单），不在支持列表里的灰掉
    fn resolution(&self, label: &str, r: (u32, u32)) -> Node {
        let on = r == self.remote_size;
        let go = if on { Go::Close } else { Go::Send(Action::ChangeResolution(r.0, r.1)) };
        Node::Entry(Entry::new(tr(label), go).dot(on).disabled(!on && !self.resolutions.contains(&r)))
    }

    /// 重新生成所有面板（节点 + 位置）：一级 + 展开的子菜单链
    fn rebuild(&mut self) {
        if !self.menu_open {
            self.panels.clear();
            return;
        }
        let mut panels: Vec<Panel> = Vec::new();
        let ids = std::iter::once(None).chain(self.open.iter().map(|&s| Some(s)));
        for id in ids {
            let nodes = self.nodes(id);
            if nodes.is_empty() {
                break;
            }
            let (rows, w, h, text_x, mark_cx) = self.layout(&nodes, if id.is_none() { MENU_W } else { SUB_W });
            let has_sub = nodes.iter().any(|n| matches!(n, Node::Entry(Entry { go: Go::Sub(_), .. })));
            let (rect, right) = match panels.last() {
                None => (self.root_rect(w, h), true),
                Some(parent) => {
                    let from = parent
                        .nodes
                        .iter()
                        .position(|n| matches!(n, Node::Entry(Entry { go: Go::Sub(s), .. }) if Some(*s) == id));
                    let Some(from) = from else { break };
                    self.child_rect(&panels, from, w, h, has_sub)
                }
            };
            panels.push(Panel {
                id,
                nodes,
                rows,
                rect,
                right,
                text_x,
                mark_cx,
            });
        }
        self.open.truncate(panels.len().saturating_sub(1));
        self.panels = panels;
    }

    /// 一个面板里各节点的位置、面板的宽高、文字起点和选中标记的位置。
    /// 对齐规则（原型）：有图标的行文字从图标右边起；一个图标都没有但有可选中的行，所有行的文字都让出黑点 / 对勾的位置；
    /// 小标题和说明不撑宽面板，说明按面板宽折行
    fn layout(&self, nodes: &[Node], min_w: f32) -> (Vec<Row>, f32, f32, f32, f32) {
        let entries = || nodes.iter().filter_map(|n| if let Node::Entry(e) = n { Some(e) } else { None });
        let (text_x, mark_cx) = if entries().any(|e| e.icon.is_some()) {
            (TEXT_X, ICON_X)
        } else if entries().any(|e| e.mark != Mark::None) {
            (SUB_DOT_TEXT_X, SUB_DOT_X)
        } else {
            (SUB_TEXT_X, SUB_DOT_X)
        };
        let width = |s: &str, px: f32| self.face.as_ref().map_or(0.0, |f| text_width(f, s, px));
        let mut w = min_w;
        for e in entries() {
            let mut need = text_x + width(&e.label, FONT_PX) + ROW_INSET + ROW_PAD;
            if let Some(h) = &e.hint {
                need += 12.0 + width(h, SMALL_PX);
            }
            if matches!(e.go, Go::Sub(_)) {
                need += 24.0;
            }
            w = w.max(need.ceil());
        }
        let mut rows = Vec::with_capacity(nodes.len());
        let mut y = PAD;
        for n in nodes {
            let (h, lines) = match n {
                Node::Entry(_) => (ROW_H, Vec::new()),
                Node::Divider => (DIVIDER_H, Vec::new()),
                Node::Head(_) => (HEAD_H, Vec::new()),
                Node::Note(t) => {
                    let lines = match &self.face {
                        Some(f) => wrap(f, t, SMALL_PX, w - 2.0 * NOTE_X),
                        None => vec![t.clone()],
                    };
                    (lines.len().max(1) as f32 * NOTE_LINE_H + NOTE_PAD_B, lines)
                }
            };
            rows.push(Row { y, h, lines });
            y += h;
        }
        (rows, w, y + PAD, text_x, mark_cx)
    }

    /// 一级面板：球下方（放不下翻到球上方），球偏右时右对齐
    fn root_rect(&self, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let (bx, by) = self.ball_pos();
        let x = if bx + BALL / 2.0 < self.screen.0 / 2.0 { bx } else { bx + BALL - w };
        let mut y = by.max(0.0) + BALL + GAP;
        if y + h > self.screen.1 {
            y = by - GAP - h;
        }
        (
            x.clamp(0.0, (self.screen.0 - w).max(0.0)),
            y.clamp(0.0, (self.screen.1 - h).max(0.0)),
            w,
            h,
        )
    }

    /// 子面板：贴着父面板、第一行对齐父面板里展开它的那一行；优先放在和父面板同一侧（向外侧展开），放不下翻到另一侧。
    /// 不压住任何祖先面板：只翻一次到另一侧会落在祖先上时（祖先正挨着那一侧），一开始就得选对方向——
    /// 所以自己还有子菜单（[has_sub]）的面板，这一侧要再留出一个二级面板的宽度，不然孙面板只能翻回来压住祖先。
    /// 返回（位置, 是不是放在了右边）
    fn child_rect(&self, panels: &[Panel], from: usize, w: f32, h: f32, has_sub: bool) -> ((f32, f32, f32, f32), bool) {
        let parent = &panels[panels.len() - 1];
        let (px, py, pw, ph) = parent.rect;
        let y = (py + parent.rows[from].y - PAD).clamp(0.0, (self.screen.1 - h).max(0.0));
        let place = |right: bool| {
            let x = if right { px + pw + PANEL_GAP } else { px - PANEL_GAP - w };
            (x, y, w, h)
        };
        // 这一侧放完还剩多少空地（负数 = 放不下）
        let room = |right: bool| if right { self.screen.0 - (px + pw + PANEL_GAP + w) } else { px - PANEL_GAP - w };
        let clear = |r: (f32, f32, f32, f32)| panels.iter().all(|p| !overlap(r, p.rect));
        let reserve = if has_sub { SUB_W + PANEL_GAP } else { 0.0 };
        // 先要求放得下、还给孙面板留位置，再放宽到只要放得下自己
        for need in [reserve, 0.0] {
            for right in [parent.right, !parent.right] {
                let r = place(right);
                if room(right) >= need && clear(r) {
                    return (r, right);
                }
            }
        }
        // 窗口窄到两侧都放不下：贴着父面板上 / 下放（优先下方、和父面板左对齐），宁可错位也不压住它
        let x = px.clamp(0.0, (self.screen.0 - w).max(0.0));
        let below = (x, py + ph + PANEL_GAP, w, h);
        let above = (x, py - PANEL_GAP - h, w, h);
        for r in [below, above] {
            if r.1 >= 0.0 && r.1 + r.3 <= self.screen.1 && clear(r) {
                return (r, parent.right);
            }
        }
        // 上下也放不下：贴在空间大的一侧（clamp 进屏幕）
        let right = room(true) >= room(false);
        let r = place(right);
        ((r.0.clamp(0.0, (self.screen.0 - w).max(0.0)), r.1, r.2, r.3), right)
    }

    fn draw_ball(&self) -> Option<LayerImage> {
        // 四周留出投影的位置
        let side = BALL + 2.0 * SHADOW;
        let mut pm = Pixmap::new(side as u32, side as u32)?;
        let (l, t) = (SHADOW, SHADOW);
        soft_shadow(&mut pm, l, t, BALL, BALL, BALL_RADIUS);
        let hot = self.hover || self.menu_open || self.press.is_some();
        fill(&mut pm, rounded(l, t, BALL, BALL, BALL_RADIUS), if hot { C_BALL_H } else { C_BALL });
        // 带状边框：一圈有宽度的带子，常态浅灰，悬停叠蓝
        let band = rounded(l + BORDER / 2.0, t + BORDER / 2.0, BALL - BORDER, BALL - BORDER, BALL_RADIUS - BORDER / 2.0);
        stroke(&mut pm, band.clone(), C_BAND, BORDER);
        if hot {
            stroke(&mut pm, band, C_BAND_H, BORDER);
        }
        // 图标：控制中心（和通用窗口标题栏的「控制中心」、全屏悬浮球同一个 Fluent 图标，20 号原尺寸），颜色不随悬停变
        draw_svg(&mut pm, &icons::BALL, l + BALL / 2.0, t + BALL / 2.0, 20.0, 1.5, C_BALL_FG);
        let (x, y) = self.ball_pos();
        Some(to_layer(&pm, (x - SHADOW).round() as i32, (y - SHADOW).round() as i32))
    }

    fn draw_menu(&self) -> Option<LayerImage> {
        let first = self.panels.first()?;
        // 所有面板画在同一张图里：取它们（连投影）的外接框
        let (mut x0, mut y0, mut x1, mut y1) = (first.rect.0, first.rect.1, first.rect.0 + first.rect.2, first.rect.1 + first.rect.3);
        for p in &self.panels {
            x0 = x0.min(p.rect.0);
            y0 = y0.min(p.rect.1);
            x1 = x1.max(p.rect.0 + p.rect.2);
            y1 = y1.max(p.rect.1 + p.rect.3);
        }
        let (ox, oy) = ((x0 - SHADOW).floor(), (y0 - SHADOW).floor());
        let mut pm = Pixmap::new((x1 + SHADOW - ox).ceil() as u32, (y1 + SHADOW - oy).ceil() as u32)?;
        let at = |p: &Panel| (p.rect.0 - ox, p.rect.1 - oy, p.rect.2, p.rect.3);

        // 所有面板的投影都先画，再画各块的底：不然子菜单的投影落在父面板上，两块之间留一条暗线（旧实现踩过）
        for p in &self.panels {
            let (x, y, w, h) = at(p);
            soft_shadow(&mut pm, x, y, w, h, PANEL_RADIUS);
        }
        for p in &self.panels {
            let (x, y, w, h) = at(p);
            fill(&mut pm, rounded(x, y, w, h, PANEL_RADIUS), C_PANEL);
        }
        for (k, p) in self.panels.iter().enumerate() {
            self.draw_panel(&mut pm, k, p, at(p));
        }
        Some(to_layer(&pm, ox as i32, oy as i32))
    }

    fn draw_panel(&self, pm: &mut Pixmap, k: usize, p: &Panel, rect: (f32, f32, f32, f32)) {
        let (x, y, w, _) = rect;
        for (i, (node, row)) in p.nodes.iter().zip(&p.rows).enumerate() {
            let ry = y + row.y;
            let cy = ry + row.h / 2.0;
            match node {
                Node::Divider => {
                    let ly = (ry + DIVIDER_H / 2.0).floor();
                    fill(pm, Rect::from_xywh(x, ly, w, 1.0).map(PathBuilder::from_rect), C_LINE);
                }
                Node::Head(text) => {
                    if let Some(face) = &self.face {
                        draw_text(pm, face, text, x + NOTE_X, ry + 6.0 + 8.0, C_TEXT2, SMALL_PX);
                    }
                }
                Node::Note(_) => {
                    if let Some(face) = &self.face {
                        for (j, line) in row.lines.iter().enumerate() {
                            let ly = ry + j as f32 * NOTE_LINE_H + NOTE_LINE_H / 2.0;
                            draw_text(pm, face, line, x + NOTE_X, ly, C_TEXT2, SMALL_PX);
                        }
                    }
                }
                Node::Entry(e) => {
                    // 悬停的行，和它展开着的子菜单入口，都有底
                    let opened = matches!(e.go, Go::Sub(s) if self.open.get(k) == Some(&s));
                    if (self.hover_node == Some((k, i)) && !e.off) || opened {
                        let (hx, hy, hw, hh) = item_rect(rect, row);
                        fill(pm, rounded(hx, hy, hw, hh, ITEM_RADIUS), C_HOVER);
                    }
                    let fg = if e.off {
                        C_DIM
                    } else if e.danger {
                        C_DANGER
                    } else {
                        C_TEXT
                    };
                    if let Some(icon) = e.icon {
                        draw_svg(pm, icon, x + ICON_X, cy, ICON, 1.5, fg);
                    }
                    // 选中标记画在图标那一列（没有图标的面板在让出的位置）；文字不变色
                    if e.on {
                        let cx = x + p.mark_cx;
                        match e.mark {
                            Mark::Dot => fill(pm, rounded(cx - DOT / 2.0, cy - DOT / 2.0, DOT, DOT, DOT / 2.0), fg),
                            Mark::Check => draw_svg(pm, &icons::CHECK, cx, cy, CHECK, 2.0, fg),
                            Mark::None => {}
                        }
                    }
                    if let Some(face) = &self.face {
                        draw_text(pm, face, &e.label, x + p.text_x, cy, fg, FONT_PX);
                        if let Some(hint) = &e.hint {
                            let hx = x + w - ROW_INSET - ROW_PAD - text_width(face, hint, SMALL_PX);
                            draw_text(pm, face, hint, hx, cy, C_TEXT2, SMALL_PX);
                        }
                    }
                    if matches!(e.go, Go::Sub(_)) {
                        draw_svg(pm, &icons::CHEVRON_RIGHT, x + w - MARK_X, cy, 16.0, 1.5, C_TEXT2);
                    }
                }
            }
        }
    }

    /// 网络状态块：固定在右下角（不跟着球走），深灰半透明底、白色等宽字（照旧实现）。
    /// 还没有数据（专业窗口刚开的第一秒）时不画
    fn draw_status(&self) -> Option<LayerImage> {
        let face = self.mono.as_ref().or(self.face.as_ref())?;
        if self.status_lines.is_empty() {
            return None;
        }
        let text_w = self
            .status_lines
            .iter()
            .map(|l| text_width(face, l, STATUS_PX))
            .fold(0.0, f32::max);
        let w = (text_w + 2.0 * STATUS_PAD_X).ceil();
        let h = self.status_lines.len() as f32 * STATUS_LINE_H + 2.0 * STATUS_PAD_Y;
        let mut pm = Pixmap::new(w as u32, h as u32)?;
        fill(&mut pm, rounded(0.0, 0.0, w, h, 4.0), C_STATUS_BG);
        for (i, line) in self.status_lines.iter().enumerate() {
            let cy = STATUS_PAD_Y + i as f32 * STATUS_LINE_H + STATUS_LINE_H / 2.0;
            draw_text(&mut pm, face, line, STATUS_PAD_X, cy, C_STATUS_FG, STATUS_PX);
        }
        // 右边只留一条缝；底边让开被控端的任务栏
        let (x, y) = (self.screen.0 - w - 1.0, self.screen.1 - h - STATUS_BOTTOM);
        Some(to_layer(&pm, x.round() as i32, y.round() as i32))
    }

    /// 指针锁定后屏幕正中的提示（原型 .dd-toast）：怎么退出被控端鼠标
    fn draw_toast(&self) -> Option<LayerImage> {
        let face = self.face.as_ref()?;
        let text = tr("Press {Ctrl+Alt} to exit the controlled side's mouse");
        let w = (text_width(face, &text, TOAST_PX) + 2.0 * TOAST_PAD_X).ceil();
        let mut pm = Pixmap::new(w as u32, TOAST_H as u32)?;
        fill(&mut pm, rounded(0.0, 0.0, w, TOAST_H, 6.0), C_TOAST_BG);
        draw_text(&mut pm, face, &text, TOAST_PAD_X, TOAST_H / 2.0, [255, 255, 255, 255], TOAST_PX);
        let (x, y) = ((self.screen.0 - w) / 2.0, (self.screen.1 - TOAST_H) / 2.0);
        Some(to_layer(&pm, x.round() as i32, y.round() as i32))
    }
}

fn tr(key: &str) -> String {
    crate::lang::translate(key.to_owned())
}

/// 和通用窗口控制中心写法一致（乘号两边有空格）
fn size_text((w, h): (u32, u32)) -> String {
    format!("{w} × {h}")
}

/// 按宽度折行：英文尽量在空格处断，中文逐字断
fn wrap(face: &Face, text: &str, px: f32, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_w = 0.0;
    for ch in text.chars() {
        let cw = text_width(face, ch.encode_utf8(&mut [0; 4]), px);
        if line_w + cw > max_w && !line.is_empty() {
            match line.rfind(' ').filter(|_| ch != ' ') {
                Some(i) => {
                    let rest = line.split_off(i + 1);
                    lines.push(line.trim_end().to_owned());
                    line = rest;
                    line_w = text_width(face, &line, px);
                }
                None => {
                    lines.push(std::mem::take(&mut line));
                    line_w = 0.0;
                }
            }
            if ch == ' ' {
                continue;
            }
        }
        line.push(ch);
        line_w += cw;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// 一行字画出来有多宽
fn text_width(face: &Face, text: &str, px: f32) -> f32 {
    let scale = px / face.units_per_em() as f32;
    text.chars()
        .map(|ch| face.glyph_hor_advance(face.glyph_index(ch).unwrap_or_default()).unwrap_or(0) as f32 * scale)
        .sum()
}

fn fill(pm: &mut Pixmap, path: Option<tiny_skia::Path>, color: [u8; 4]) {
    if let Some(p) = path {
        pm.fill_path(&p, &paint(color), FillRule::Winding, Transform::identity(), None);
    }
}

fn stroke(pm: &mut Pixmap, path: Option<tiny_skia::Path>, color: [u8; 4], width: f32) {
    if let Some(p) = path {
        let s = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pm.stroke_path(&p, &paint(color), &s, Transform::identity(), None);
    }
}

/// 柔和投影：同心圆角矩形叠低透明度（旧实现的画法，比真高斯便宜，视觉上够用），整体往下偏 1 像素
fn soft_shadow(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let n = SHADOW as i32;
    for i in (1..=n).rev() {
        let a = (0.020 * (n - i + 1) as f32 / n as f32 * 255.0).round() as u8;
        let g = i as f32;
        fill(pm, rounded(x - g, y - g + 1.0, w + 2.0 * g, h + 2.0 * g, r + g), [0, 0, 0, a]);
    }
}

/// 以 (cx, cy) 为中心、边长 `size` 画一个图标：描边图标线宽 `stroke`（图标格子里的单位，和通用窗口一样随尺寸缩放；
/// 图标一般 1.5，菜单里的小对勾照原型用 2）、圆头圆角
fn draw_svg(pm: &mut Pixmap, icon: &icons::Icon, cx: f32, cy: f32, size: f32, stroke: f32, color: [u8; 4]) {
    let mut pb = PathBuilder::new();
    for seg in icon.segs {
        match *seg {
            icons::Seg::M(x, y) => pb.move_to(x, y),
            icons::Seg::L(x, y) => pb.line_to(x, y),
            icons::Seg::C(x1, y1, x2, y2, x, y) => pb.cubic_to(x1, y1, x2, y2, x, y),
            icons::Seg::Z => pb.close(),
        }
    }
    let Some(path) = pb.finish() else {
        return;
    };
    let k = size / icon.grid;
    let t = Transform::from_row(k, 0.0, 0.0, k, cx - size / 2.0, cy - size / 2.0);
    if icon.fill {
        pm.fill_path(&path, &paint(color), FillRule::Winding, t, None);
    } else {
        let s = Stroke {
            width: stroke,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(color), &s, t, None);
    }
}

/// 「显示屏 1」（和通用窗口的屏幕标签、控制中心同一个词条）
fn screen_name(index: i32) -> String {
    crate::lang::translate(format!("Screen {{{}}}", index + 1))
}

fn inside((px, py): (f32, f32), x: f32, y: f32, w: f32, h: f32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// 面板（x, y, 宽, 高）里一行的悬停块 / 点击区：左右各让出 [`ROW_INSET`]，上下就是这一行的 `y` / `h`
/// （第一行从面板顶往下 [`PAD`]、最后一行离面板底 [`PAD`]，由 `layout` 排）
fn item_rect(panel: (f32, f32, f32, f32), row: &Row) -> (f32, f32, f32, f32) {
    (panel.0 + ROW_INSET, panel.1 + row.y, panel.2 - 2.0 * ROW_INSET, row.h)
}

/// 两个矩形（x, y, 宽, 高）有重叠；只是挨着不算
fn overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

fn paint([r, g, b, a]: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(r, g, b, a);
    p.anti_alias = true;
    p
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    Rect::from_xywh(x, y, w, h)?;
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// tiny-skia 是预乘的 R G B A，Wayland 的 ARGB8888 在内存里是 B G R A（同样预乘）
fn to_layer(pm: &Pixmap, x: i32, y: i32) -> LayerImage {
    let mut bgra = pm.data().to_vec();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    LayerImage {
        x,
        y,
        width: pm.width(),
        height: pm.height(),
        bgra,
    }
}

/// 字形轮廓（ttf-parser）转成 tiny-skia 的路径（照 `src/whiteboard/win_linux.rs`）
struct Outline<'a> {
    pb: &'a mut PathBuilder,
    t: Transform,
}

impl Outline<'_> {
    fn map(&self, x: f32, y: f32) -> Point {
        let mut p = Point::from_xy(x, y);
        self.t.map_point(&mut p);
        p
    }
}

impl ttf_parser::OutlineBuilder for Outline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.pb.move_to(p.x, p.y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.pb.line_to(p.x, p.y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (p1, p) = (self.map(x1, y1), self.map(x, y));
        self.pb.quad_to(p1.x, p1.y, p.x, p.y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (p1, p2, p) = (self.map(x1, y1), self.map(x2, y2), self.map(x, y));
        self.pb.cubic_to(p1.x, p1.y, p2.x, p2.y, p.x, p.y);
    }

    fn close(&mut self) {
        self.pb.close();
    }
}

/// 从左边 `x` 起画一行字，竖直方向以 `cy` 为中线
fn draw_text(pm: &mut Pixmap, face: &Face, text: &str, x: f32, cy: f32, color: [u8; 4], px: f32) {
    let scale = px / face.units_per_em() as f32;
    let baseline = cy + (face.ascender() as f32 + face.descender() as f32) * scale / 2.0;
    let mut pb = PathBuilder::new();
    let mut pen = x;
    for ch in text.chars() {
        let g = face.glyph_index(ch).unwrap_or_default();
        let mut o = Outline {
            pb: &mut pb,
            t: Transform::from_translate(pen, baseline).pre_scale(scale, -scale),
        };
        face.outline_glyph(g, &mut o);
        pen += face.glyph_hor_advance(g).unwrap_or(0) as f32 * scale;
    }
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, &paint(color), FillRule::Winding, Transform::identity(), None);
    }
}

/// 菜单字体（优先系统里的中文字体，菜单是中文时要有中文字形，找不到再用无衬线字体）和状态块的等宽字体。
/// 专业窗口是一次会话一个的进程，字体数据读进来后一直用到进程结束，所以直接 leak（白板功能同样做法）。
fn load_faces() -> (Option<Face<'static>>, Option<Face<'static>>) {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let ui = [
        fontdb::Family::Name("Noto Sans CJK SC"),
        fontdb::Family::Name("Noto Sans SC"),
        fontdb::Family::Name("WenQuanYi Micro Hei"),
        fontdb::Family::Name("Droid Sans Fallback"),
        fontdb::Family::SansSerif,
    ];
    // 旧实现用 Consolas，装机不带时落到 Liberation Mono
    let mono = [
        fontdb::Family::Name("Liberation Mono"),
        fontdb::Family::Name("DejaVu Sans Mono"),
        fontdb::Family::Monospace,
    ];
    // DemiLight（350）：常规字重在小字号下发闷（旧实现的结论），没有这个字重时取最接近的
    let load = |families: &[fontdb::Family], weight: u16| match load_face(&db, families, weight) {
        Ok(f) => Some(f),
        Err(e) => {
            log::warn!("ddpro: pro view font: {e}");
            None
        }
    };
    (load(&ui, 350), load(&mono, 400))
}

fn load_face(db: &fontdb::Database, families: &[fontdb::Family], weight: u16) -> Result<Face<'static>, String> {
    let id = families
        .iter()
        .find_map(|f| {
            db.query(&fontdb::Query {
                families: std::slice::from_ref(f),
                weight: fontdb::Weight(weight),
                ..fontdb::Query::default()
            })
        })
        .ok_or("no usable font")?;
    let (source, index) = db.face_source(id).ok_or("no face for the chosen font")?;
    let data: &'static [u8] = Box::leak(
        match source {
            fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => {
                std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?
            }
            fontdb::Source::Binary(data) => data.as_ref().as_ref().to_vec(),
        }
        .into_boxed_slice(),
    );
    Face::parse(data, index).map_err(|e| e.to_string())
}
