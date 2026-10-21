//! 主控端专业窗口的悬浮球和菜单（P1 第 2 项「专业会话只有一个窗口」）：专业会话期间通用窗口隐藏，
//! 操作入口都在这里。外观和手感沿用旧 DreamDesk 用户已经习惯的（42 像素圆角方块、默认缩在右上角顶边、可拖动贴边、
//! 点开两级菜单），改掉审查发现的问题（P1 实施记录第 2 节）：动作在松开时才执行；鼠标在球 / 菜单上时移动、按键、
//! 滚轮都不发给远程；Esc 关菜单；只在状态变化时重画、只画球和菜单那么大。
//!
//! 样式（尺寸、配色、投影、图标、字体）照旧 DreamDesk 的 `client/src/dd_ui_ball.c`（用户 2026-10-20 要求）。
//! 右下角的网络状态块（菜单开关，默认隐藏）显示的是 RustDesk 质量面板在专业模式下的同一份实测值。
//!
//! 本模块只管布局、绘制和交互；画出来的图交给专业窗口的叠加图层（`ddpro::view::Window::set_layer`）。
//! 画法照 RustDesk 白板功能（`src/whiteboard`）：tiny-skia 画形状，ttf-parser 取字形轮廓。
//! 屏幕缩放不是 100% 时专业画面本来就不逐像素（质量面板会标「Scaled」），这里按物理像素画、不跟缩放。

use ddpro::view::LayerImage;
use hbb_common::log;
use std::time::{Duration, Instant};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Point, Rect, Stroke, Transform};
use ttf_parser::Face;

/// 叠加图层的编号
pub const LAYER_BALL: usize = 0;
pub const LAYER_MENU: usize = 1;
pub const LAYER_STATUS: usize = 2;

const BALL: f32 = 42.0;
const BALL_RADIUS: f32 = 13.0;
const BORDER: f32 = 4.0;
/// 缩进时露出的宽度
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
// 面板尺寸、配色、字号都照旧 DreamDesk（`client/src/dd_ui_ball.c`，浅色）
const MENU_W: f32 = 238.0;
const SUB_W: f32 = 200.0;
const ROW_H: f32 = 34.0;
const PAD: f32 = 6.0;
const PANEL_RADIUS: f32 = 10.0;
/// 投影向外扩散的像素（球和面板都有）
const SHADOW: f32 = 10.0;
const GAP: f32 = 8.0;
/// 图标中心、文字左起距面板左边
const ICON_X: f32 = 16.0;
const TEXT_X: f32 = 40.0;
/// 11 号字（Pango 的 11pt ≈ 14.7 像素），思源黑体 DemiLight
const FONT_PX: f32 = 14.67;

const C_PANEL: [u8; 4] = [255, 255, 255, 250];
const C_TEXT: [u8; 4] = [41, 41, 46, 255];
const C_DIM: [u8; 4] = [140, 140, 148, 255];
const C_HOVER: [u8; 4] = [237, 237, 240, 255];
const C_DANGER: [u8; 4] = [217, 61, 61, 255];
/// 球：常态浅灰底 + 浅灰带状边框；悬停时底转白、边框叠蓝。图标颜色始终不变
const C_BALL: [u8; 4] = [241, 241, 242, 247];
const C_BALL_H: [u8; 4] = [255, 255, 255, 252];
const C_BAND: [u8; 4] = [227, 227, 230, 255];
const C_BAND_H: [u8; 4] = [177, 203, 255, 255];
const C_BALL_FG: [u8; 4] = [71, 77, 87, 255];
/// 网络状态块：10 号等宽字（≈13.3 像素），深灰 72% 底、白字 92%；最窄 132 像素，离屏幕底边 110 像素（让开被控端任务栏）
const STATUS_PX: f32 = 13.33;
const STATUS_MIN_W: f32 = 132.0;
const STATUS_BOTTOM: f32 = 110.0;
const C_STATUS_BG: [u8; 4] = [87, 87, 94, 184];
const C_STATUS_FG: [u8; 4] = [255, 255, 255, 235];

/// Linux evdev 码
const BTN_LEFT: u32 = 0x110;
const KEY_ESC: u32 = 1;

/// 菜单里选中的动作，由专业窗口交给会话进程（「断开连接」由专业窗口自己按用户关窗结束）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SwitchDisplay(i32),
    /// 网络状态块开 / 关之后的状态（会话进程记下来，换屏开新窗口时照旧）
    ShowStatus(bool),
    GeneralMode,
    CtrlAltDel,
    LockScreen,
    Disconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Top,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    SwitchDisplay,
    Status,
    General,
    CtrlAltDel,
    Lock,
    Disconnect,
}

const ITEMS: [Item; 6] = [Item::SwitchDisplay, Item::Status, Item::General, Item::CtrlAltDel, Item::Lock, Item::Disconnect];

impl Item {
    /// 菜单文字尽量用 RustDesk 自己菜单里的同一批词条，叫法一致、不新增翻译（切换模式是我们独有的）
    fn label(self) -> String {
        crate::lang::translate(
            match self {
                Item::SwitchDisplay => "Switch display",
                // 状态块里就是 RustDesk 质量面板在专业模式下显示的那份实测值，叫法跟它一致
                Item::Status => "Show quality monitor",
                Item::General => "Switch to general mode",
                Item::CtrlAltDel => "Insert Ctrl + Alt + Del",
                Item::Lock => "Insert Lock",
                Item::Disconnect => "Disconnect",
            }
            .to_owned(),
        )
    }

    fn has_sub(self) -> bool {
        self == Item::SwitchDisplay
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Ball,
    Row(usize),
    SubRow(usize),
    /// 菜单开着时点在外面：关菜单，这一下不发给远程
    Outside,
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
    status_lines: Vec<String>,
    status_dirty: bool,
    /// 球完全露出时的左上角
    x: f32,
    y: f32,
    edge: Option<Edge>,
    tucked: bool,
    hover: bool,
    menu_open: bool,
    hover_row: Option<usize>,
    sub_open: Option<usize>,
    hover_sub: Option<usize>,
    press: Option<Press>,
    /// 按下时落在球 / 菜单上（被拦下）的按键：松开也拦下
    swallowed: Vec<u32>,
    esc_swallowed: bool,
    pointer: (f32, f32),
    free_since: Option<Instant>,
    display_count: u32,
    display_index: i32,
    dirty: bool,
}

impl Ball {
    pub fn new((w, h): (u32, u32), display_count: u32, display_index: i32, show_status: bool) -> Self {
        let (face, mono) = load_faces();
        if face.is_none() {
            log::warn!("ddpro: pro view menu has no font, showing no text");
        }
        let screen = (w as f32, h as f32);
        Self {
            screen,
            face,
            mono,
            show_status,
            status_lines: Vec::new(),
            status_dirty: show_status,
            x: (screen.0 - RIGHT_GAP - BALL).max(0.0),
            y: 0.0,
            edge: Some(Edge::Top),
            tucked: true,
            hover: false,
            menu_open: false,
            hover_row: None,
            sub_open: None,
            hover_sub: None,
            press: None,
            swallowed: Vec::new(),
            esc_swallowed: false,
            pointer: (-1.0, -1.0),
            free_since: None,
            display_count: display_count.max(1),
            display_index,
            dirty: true,
        }
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
    }

    /// 鼠标移动；返回 true 表示这一下归悬浮球，不发给远程
    pub fn pointer_motion(&mut self, x: f64, y: f64) -> bool {
        self.pointer = (x as f32, y as f32);
        if let Some(p) = self.press.as_mut() {
            if p.target == Target::Ball {
                let (dx, dy) = (self.pointer.0 - p.at.0, self.pointer.1 - p.at.1);
                if !p.dragging && (dx.abs() > DRAG_SLOP || dy.abs() > DRAG_SLOP) {
                    p.dragging = true;
                    self.menu_open = false;
                    self.sub_open = None;
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
        self.update_hover();
        self.press.is_some() || self.over_ui()
    }

    /// 鼠标按键；返回（是否归悬浮球，选中的动作）
    pub fn pointer_button(&mut self, button: u32, down: bool, x: f64, y: f64) -> (bool, Option<Action>) {
        self.pointer = (x as f32, y as f32);
        if down {
            let target = if self.hit_ball() {
                Target::Ball
            } else if let Some(r) = self.row_at() {
                Target::Row(r)
            } else if let Some(r) = self.sub_row_at() {
                Target::SubRow(r)
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
                    self.menu_open = true;
                }
                None
            }
            // 松开时还在按下的那一行上才算选中（按下后挪开就是取消）
            Target::Row(r) if self.row_at() == Some(r) => {
                let item = ITEMS[r];
                if item.has_sub() {
                    self.sub_open = if self.sub_open == Some(r) { None } else { Some(r) };
                    None
                } else {
                    self.close_menu();
                    Some(match item {
                        Item::Status => {
                            self.show_status = !self.show_status;
                            self.status_dirty = true;
                            Action::ShowStatus(self.show_status)
                        }
                        Item::General => Action::GeneralMode,
                        Item::CtrlAltDel => Action::CtrlAltDel,
                        Item::Lock => Action::LockScreen,
                        _ => Action::Disconnect,
                    })
                }
            }
            Target::SubRow(r) if self.sub_row_at() == Some(r) => {
                self.close_menu();
                (r as i32 != self.display_index).then_some(Action::SwitchDisplay(r as i32))
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
    }

    /// 滚轮：鼠标在球 / 菜单上时不发给远程
    pub fn wheel(&self) -> bool {
        self.over_ui()
    }

    /// 键盘：菜单开着时 Esc 关菜单（按下和松开都不发给远程）
    pub fn key(&mut self, evdev: u32, down: bool) -> bool {
        if evdev != KEY_ESC {
            return false;
        }
        if down && self.menu_open {
            self.close_menu();
            self.esc_swallowed = true;
            return true;
        }
        if !down && self.esc_swallowed {
            self.esc_swallowed = false;
            return true;
        }
        false
    }

    /// 定时检查：停在屏幕中间、没人碰一会儿就回到顶边
    pub fn tick(&mut self, now: Instant) {
        let idle = self.edge.is_none() && !self.menu_open && self.press.is_none() && !self.hover;
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
        if std::mem::take(&mut self.status_dirty) {
            out.push((LAYER_STATUS, if self.show_status { self.draw_status() } else { None }));
        }
        out
    }

    fn close_menu(&mut self) {
        if self.menu_open || self.sub_open.is_some() {
            self.dirty = true;
        }
        self.menu_open = false;
        self.sub_open = None;
        self.hover_row = None;
        self.hover_sub = None;
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

    /// 球此刻画在哪（缩进时只露一条边）
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
        let row = self.row_at();
        let sub = self.sub_row_at();
        if hover != self.hover || row != self.hover_row || sub != self.hover_sub {
            self.dirty = true;
        }
        self.hover = hover;
        self.hover_row = row;
        self.hover_sub = sub;
        // 移到带子菜单的行就展开；移到别的行就收起（移进子菜单本身不收）
        if let Some(r) = row {
            let want = ITEMS[r].has_sub().then_some(r);
            if want != self.sub_open {
                self.sub_open = want;
                self.dirty = true;
            }
        }
    }

    fn over_ui(&self) -> bool {
        self.hit_ball() || (self.menu_open && (self.in_rect(self.main_rect()) || self.sub_rect().is_some_and(|r| self.in_rect(r))))
    }

    fn in_rect(&self, (x, y, w, h): (f32, f32, f32, f32)) -> bool {
        inside(self.pointer, x, y, w, h)
    }

    /// 主菜单面板（不含投影）
    fn main_rect(&self) -> (f32, f32, f32, f32) {
        let (bx, by) = self.ball_pos();
        let h = ITEMS.len() as f32 * ROW_H + 2.0 * PAD;
        let x = if bx + BALL / 2.0 < self.screen.0 / 2.0 { bx } else { bx + BALL - MENU_W };
        let mut y = by.max(0.0) + BALL + GAP;
        if y + h > self.screen.1 {
            y = by - GAP - h;
        }
        (
            x.clamp(0.0, (self.screen.0 - MENU_W).max(0.0)),
            y.clamp(0.0, (self.screen.1 - h).max(0.0)),
            MENU_W,
            h,
        )
    }

    fn sub_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let r = self.sub_open?;
        let (mx, my, mw, _) = self.main_rect();
        let h = self.display_count as f32 * ROW_H + 2.0 * PAD;
        let x = if mx + mw + 4.0 + SUB_W <= self.screen.0 { mx + mw + 4.0 } else { mx - 4.0 - SUB_W };
        let y = (my + r as f32 * ROW_H).clamp(0.0, (self.screen.1 - h).max(0.0));
        Some((x.max(0.0), y, SUB_W, h))
    }

    fn row_at(&self) -> Option<usize> {
        if !self.menu_open {
            return None;
        }
        let (x, y, w, h) = self.main_rect();
        if !inside(self.pointer, x, y + PAD, w, h - 2.0 * PAD) {
            return None;
        }
        let r = ((self.pointer.1 - y - PAD) / ROW_H) as usize;
        (r < ITEMS.len()).then_some(r)
    }

    fn sub_row_at(&self) -> Option<usize> {
        let (x, y, w, h) = self.sub_rect()?;
        if !inside(self.pointer, x, y + PAD, w, h - 2.0 * PAD) {
            return None;
        }
        let r = ((self.pointer.1 - y - PAD) / ROW_H) as usize;
        (r < self.display_count as usize).then_some(r)
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
        // 图标：两条上下叠的胶囊，各带一个错开的小圆点，颜色不随悬停变
        let (cw, ch, gap) = (17.0, 6.6, 3.0);
        let (cx, cy) = (l + BALL / 2.0, t + BALL / 2.0);
        let (x0, y0) = (cx - cw / 2.0, cy - (ch * 2.0 + gap) / 2.0);
        for r in 0..2 {
            let ry = y0 + r as f32 * (ch + gap);
            stroke(&mut pm, rounded(x0, ry, cw, ch, ch / 2.0), C_BALL_FG, 1.4);
            let dx = if r == 0 { x0 + cw - ch / 2.0 } else { x0 + ch / 2.0 };
            fill(&mut pm, PathBuilder::from_circle(dx, ry + ch / 2.0, 1.4), C_BALL_FG);
        }
        let (x, y) = self.ball_pos();
        Some(to_layer(&pm, (x - SHADOW).round() as i32, (y - SHADOW).round() as i32))
    }

    fn draw_menu(&self) -> Option<LayerImage> {
        let main = self.main_rect();
        let sub = self.sub_rect();
        // 两块面板画在同一张图里：取它们（连投影）的外接框
        let (mut x0, mut y0, mut x1, mut y1) = (main.0, main.1, main.0 + main.2, main.1 + main.3);
        if let Some(s) = sub {
            x0 = x0.min(s.0);
            y0 = y0.min(s.1);
            x1 = x1.max(s.0 + s.2);
            y1 = y1.max(s.1 + s.3);
        }
        let (ox, oy) = ((x0 - SHADOW).floor(), (y0 - SHADOW).floor());
        let mut pm = Pixmap::new((x1 + SHADOW - ox).ceil() as u32, (y1 + SHADOW - oy).ceil() as u32)?;
        let at = |r: (f32, f32, f32, f32)| (r.0 - ox, r.1 - oy, r.2, r.3);
        let (main, sub) = (at(main), sub.map(at));

        // 两块的投影都先画，再画两块的底：不然子菜单的投影落在父面板上，两块之间留一条暗线（旧实现踩过）
        soft_shadow(&mut pm, main.0, main.1, main.2, main.3, PANEL_RADIUS);
        if let Some(s) = sub {
            soft_shadow(&mut pm, s.0, s.1, s.2, s.3, PANEL_RADIUS);
        }
        fill(&mut pm, rounded(main.0, main.1, main.2, main.3, PANEL_RADIUS), C_PANEL);
        if let Some(s) = sub {
            fill(&mut pm, rounded(s.0, s.1, s.2, s.3, PANEL_RADIUS), C_PANEL);
        }

        let (x, y, w, _) = main;
        for (i, item) in ITEMS.iter().enumerate() {
            let ry = y + PAD + i as f32 * ROW_H;
            if self.hover_row == Some(i) || self.sub_open == Some(i) {
                fill(&mut pm, rounded(x + 4.0, ry, w - 8.0, ROW_H - 2.0, 7.0), C_HOVER);
            }
            let fg = if *item == Item::Disconnect { C_DANGER } else { C_TEXT };
            let cy = ry + ROW_H / 2.0;
            draw_icon(&mut pm, *item, x + ICON_X, cy, fg);
            if let Some(face) = &self.face {
                draw_text(&mut pm, face, &item.label(), x + TEXT_X, cy, fg, FONT_PX);
            }
            if item.has_sub() {
                chevron(&mut pm, x + w - 16.0, cy, C_DIM);
            }
            // 开关项：开着时右边一个对勾
            if *item == Item::Status && self.show_status {
                let mut pb = PathBuilder::new();
                pb.move_to(x + w - 21.0, cy);
                pb.line_to(x + w - 17.5, cy + 3.5);
                pb.line_to(x + w - 11.0, cy - 4.0);
                stroke(&mut pm, pb.finish(), C_TEXT, 1.6);
            }
        }

        if let Some((sx, sy, sw, _)) = sub {
            let monitor = crate::lang::translate("Monitor".to_owned());
            for i in 0..self.display_count as usize {
                let ry = sy + PAD + i as f32 * ROW_H;
                if self.hover_sub == Some(i) {
                    fill(&mut pm, rounded(sx + 4.0, ry, sw - 8.0, ROW_H - 2.0, 7.0), C_HOVER);
                }
                let cy = ry + ROW_H / 2.0;
                // 当前看的那块屏前面一个圆点
                if i as i32 == self.display_index {
                    fill(&mut pm, PathBuilder::from_circle(sx + 18.0, cy, 3.2), C_TEXT);
                }
                if let Some(face) = &self.face {
                    draw_text(&mut pm, face, &format!("{monitor} {}", i + 1), sx + 34.0, cy, C_TEXT, FONT_PX);
                }
            }
        }
        Some(to_layer(&pm, ox as i32, oy as i32))
    }

    /// 网络状态块：固定在右下角（不跟着球走），深灰半透明底、白色等宽字（照旧实现）。
    /// 还没有数据（专业窗口刚开的第一秒）时不画
    fn draw_status(&self) -> Option<LayerImage> {
        let face = self.mono.as_ref().or(self.face.as_ref())?;
        if self.status_lines.is_empty() {
            return None;
        }
        let (line_h, pad) = (22.0, 12.0);
        let text_w = self
            .status_lines
            .iter()
            .map(|l| text_width(face, l, STATUS_PX))
            .fold(0.0, f32::max);
        let w = (text_w + 2.0 * pad).ceil().max(STATUS_MIN_W);
        let h = self.status_lines.len() as f32 * line_h + 2.0 * pad;
        let mut pm = Pixmap::new(w as u32, h as u32)?;
        fill(&mut pm, rounded(0.0, 0.0, w, h, 6.0), C_STATUS_BG);
        for (i, line) in self.status_lines.iter().enumerate() {
            draw_text(&mut pm, face, line, pad, pad + i as f32 * line_h + line_h / 2.0, C_STATUS_FG, STATUS_PX);
        }
        // 右边只留一条缝；底边让开被控端的任务栏
        let (x, y) = (self.screen.0 - w - 1.0, self.screen.1 - h - STATUS_BOTTOM);
        Some(to_layer(&pm, x.round() as i32, y.round() as i32))
    }
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

/// 子菜单箭头 ›
fn chevron(pm: &mut Pixmap, cx: f32, cy: f32, color: [u8; 4]) {
    let mut pb = PathBuilder::new();
    pb.move_to(cx - 2.5, cy - 4.0);
    pb.line_to(cx + 2.0, cy);
    pb.line_to(cx - 2.5, cy + 4.0);
    stroke(pm, pb.finish(), color, 1.6);
}

/// 每项左边的线性单色小图标（照旧实现的风格，用路径画，不用图标资源）
fn draw_icon(pm: &mut Pixmap, item: Item, cx: f32, cy: f32, color: [u8; 4]) {
    let lw = 1.5;
    match item {
        // 屏幕 + 右向箭头（旧实现「切换下一个屏幕」的图标）
        Item::SwitchDisplay => {
            stroke(pm, rounded(cx - 7.0, cy - 5.5, 11.0, 10.0, 1.6), color, lw);
            let mut pb = PathBuilder::new();
            pb.move_to(cx + 2.0, cy);
            pb.line_to(cx + 7.0, cy);
            pb.move_to(cx + 4.5, cy - 2.5);
            pb.line_to(cx + 7.0, cy);
            pb.line_to(cx + 4.5, cy + 2.5);
            stroke(pm, pb.finish(), color, lw);
        }
        // 键盘：圆角外框 + 两排键点 + 一条空格键
        Item::CtrlAltDel => {
            stroke(pm, rounded(cx - 7.5, cy - 5.0, 15.0, 10.0, 1.8), color, lw);
            for row in 0..2 {
                for col in 0..4 {
                    let (kx, ky) = (cx - 4.5 + col as f32 * 3.0, cy - 2.2 + row as f32 * 2.6);
                    fill(pm, PathBuilder::from_circle(kx, ky, 0.75), color);
                }
            }
            let mut pb = PathBuilder::new();
            pb.move_to(cx - 3.0, cy + 2.8);
            pb.line_to(cx + 3.0, cy + 2.8);
            stroke(pm, pb.finish(), color, 1.2);
        }
        // 挂锁：锁身 + 半圆锁梁 + 锁孔
        Item::Lock => {
            stroke(pm, rounded(cx - 5.5, cy - 1.0, 11.0, 8.0, 1.6), color, lw);
            let mut pb = PathBuilder::new();
            pb.move_to(cx - 3.5, cy - 1.0);
            pb.line_to(cx - 3.5, cy - 3.5);
            pb.quad_to(cx - 3.5, cy - 7.5, cx, cy - 7.5);
            pb.quad_to(cx + 3.5, cy - 7.5, cx + 3.5, cy - 3.5);
            pb.line_to(cx + 3.5, cy - 1.0);
            stroke(pm, pb.finish(), color, lw);
            fill(pm, PathBuilder::from_circle(cx, cy + 3.0, 1.1), color);
        }
        // 信号条（旧实现「网络状态」的图标）
        Item::Status => {
            for i in 0..3 {
                let bh = 3.0 + i as f32 * 3.0;
                fill(pm, Rect::from_xywh(cx - 6.0 + i as f32 * 4.5, cy + 5.0 - bh, 2.6, bh).map(PathBuilder::from_rect), color);
            }
        }
        // 上下两条反向箭头：在两种模式之间切换
        Item::General => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx - 6.5, cy - 3.0);
            pb.line_to(cx + 6.5, cy - 3.0);
            pb.move_to(cx + 4.0, cy - 5.5);
            pb.line_to(cx + 6.5, cy - 3.0);
            pb.line_to(cx + 4.0, cy - 0.5);
            pb.move_to(cx + 6.5, cy + 3.5);
            pb.line_to(cx - 6.5, cy + 3.5);
            pb.move_to(cx - 4.0, cy + 1.0);
            pb.line_to(cx - 6.5, cy + 3.5);
            pb.line_to(cx - 4.0, cy + 6.0);
            stroke(pm, pb.finish(), color, lw);
        }
        // 向右出门的箭头（旧实现「退出远控」的图标）
        Item::Disconnect => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx + 1.0, cy - 6.0);
            pb.line_to(cx - 6.0, cy - 6.0);
            pb.line_to(cx - 6.0, cy + 6.0);
            pb.line_to(cx + 1.0, cy + 6.0);
            pb.move_to(cx + 2.0, cy);
            pb.line_to(cx + 7.0, cy);
            pb.move_to(cx + 4.5, cy - 2.5);
            pb.line_to(cx + 7.0, cy);
            pb.line_to(cx + 4.5, cy + 2.5);
            stroke(pm, pb.finish(), color, lw);
        }
    }
}

fn inside((px, py): (f32, f32), x: f32, y: f32, w: f32, h: f32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
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
