//! 全屏 Wayland 窗口：导入显存画面（dmabuf）、提交、收「已释放」和「已上屏」回执；收键鼠（键盘取原始键码，
//! 不按键盘布局翻译：RustDesk 的映射键盘模式只要物理键码）；把远程光标设成本窗口的系统指针。
//!
//! 调用方自己驱动循环：`present` 提交一张画面，`dispatch` 等事件（带超时）。画面轮流用一组槽位，
//! 合成器还拿着的槽位（没回「已释放」）不能再写，[`Window::free_slot`] 只给空闲的。

use ddpro_dec::Exported;
use sctk::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_dmabuf, delegate_output, delegate_pointer, delegate_presentation_time,
    delegate_registry, delegate_seat, delegate_shm, delegate_subcompositor, delegate_xdg_shell, delegate_xdg_window,
    dmabuf::{DmabufFeedback, DmabufHandler, DmabufState},
    output::{OutputHandler, OutputState},
    presentation_time::{PresentTime, PresentationTimeHandler, PresentationTimeState},
    reexports::{
        client::{
            backend::{ObjectId, WaylandError},
            globals::registry_queue_init,
            protocol::{
                wl_buffer, wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_subsurface::WlSubsurface, wl_surface,
            },
            Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
        },
        protocols::wp::{
            idle_inhibit::zv1::client::{
                zwp_idle_inhibit_manager_v1::{self, ZwpIdleInhibitManagerV1},
                zwp_idle_inhibitor_v1::{self, ZwpIdleInhibitorV1},
            },
            keyboard_shortcuts_inhibit::zv1::client::{
                zwp_keyboard_shortcuts_inhibit_manager_v1::{self, ZwpKeyboardShortcutsInhibitManagerV1},
                zwp_keyboard_shortcuts_inhibitor_v1::{self, ZwpKeyboardShortcutsInhibitorV1},
            },
            linux_dmabuf::zv1::client::{
                zwp_linux_buffer_params_v1::{self, Flags},
                zwp_linux_dmabuf_feedback_v1::{self, TrancheFlags},
            },
            presentation_time::client::wp_presentation_feedback::{self, Kind},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
    shell::{
        xdg::{
            window::{Window as XdgWindow, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell, XdgSurface,
        },
        WaylandSurface,
    },
    subcompositor::SubcompositorState,
};
use std::{
    collections::HashMap,
    fmt,
    io::ErrorKind,
    mem,
    os::fd::{AsFd, AsRawFd, BorrowedFd},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn err<E: fmt::Display>(what: &str) -> impl FnOnce(E) -> Error + '_ {
    move |e| Error(format!("{what}: {e}"))
}

/// 本机单调时钟（纳秒）。上屏回执用的也是它（合成器报的时钟号不是它时 [`Event::Presented`] 里会标出来）。
pub fn now_ns() -> i64 {
    clock_ns(libc::CLOCK_MONOTONIC)
}

fn clock_ns(clock: libc::clockid_t) -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(clock, &mut ts) };
    ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64
}

#[derive(Debug, Clone)]
pub enum Event {
    /// 这一帧开始显示了。`zero_copy`：合成器没有再合成一遍，直接把我们的画面送去显示
    Presented {
        slot: usize,
        commit_ns: i64,
        present_ns: i64,
        zero_copy: bool,
        /// 合成器报的时钟不是本机单调时钟，`present_ns` 不能和 `commit_ns` 直接相减
        foreign_clock: bool,
    },
    /// 这一帧被合成器跳过了（下一帧来得太快，或窗口不可见）
    Discarded { slot: usize },
    Resized { width: u32, height: u32 },
    Closed,
    /// 按键：Linux evdev 键码（X 键码 = evdev + 8）
    Key { evdev: u32, down: bool },
    /// 修饰键 / 锁定键状态（XKB 修饰位：depressed 是按住的，locked 是锁定的）
    Modifiers { depressed: u32, locked: u32 },
    /// 键盘焦点离开窗口：还按着的键要替用户抬起，免得远程那边一直按着
    KeyboardLeave,
    /// 鼠标移动（窗口坐标）
    PointerMotion { x: f64, y: f64 },
    /// 鼠标按键（Linux BTN_* 码），带按下时的窗口坐标
    PointerButton { button: u32, down: bool, x: f64, y: f64 },
    /// 指针离开窗口或某个叠加图层。按着键时合成器把指针抢走（比如弹出系统对话框）也是这个，之后收不到那次松开
    PointerLeave,
    /// 滚轮格数（往下 / 往右为正，和 Wayland 一致）
    Wheel { dx: i32, dy: i32 },
    /// 合成器报告系统快捷键是否已经让给本窗口（Win、Alt+Tab 等发给远程）：用户在合成器的确认框里允许、
    /// 窗口拿到键盘焦点时为 true；拒绝或失去焦点时为 false
    ShortcutsInhibited(bool),
}

/// 一个光标图案（RGBA，未预乘）。
#[derive(Debug, Clone)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    pub hotx: u32,
    pub hoty: u32,
    pub rgba: Vec<u8>,
}

/// 合成器对这个窗口说的格式支持：能用的（格式, modifier），以及其中能直接送显示硬件的。
#[derive(Debug, Default, Clone)]
pub struct FormatSupport {
    pub usable: Vec<(u32, u64)>,
    pub scanout: Vec<(u32, u64)>,
}

struct Slot {
    buffer: Option<wl_buffer::WlBuffer>,
    /// 建这个缓冲用的参数对象：留到缓冲换掉时才销毁——立刻销毁就收不到合成器「导入失败」的事件
    params: Option<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1>,
    busy: bool,
}

impl Slot {
    fn clear(&mut self) {
        if let Some(b) = self.buffer.take() {
            b.destroy();
        }
        if let Some(p) = self.params.take() {
            p.destroy();
        }
    }
}

struct Commit {
    slot: usize,
    commit_ns: i64,
}

struct State {
    registry: RegistryState,
    outputs: OutputState,
    _compositor: CompositorState,
    _xdg: XdgShell,
    dmabuf: DmabufState,
    presentation: PresentationTimeState,
    window: XdgWindow,
    /// 窗口在屏上时不让桌面因「没有本机键鼠操作」而黑屏 / 锁屏：远程会话里本机常常长时间没有输入
    /// （比如盯着远程渲染），GNOME 默认几分钟就会黑屏（2026-10-13 瘦客户机 10 分钟测试第 4 分钟实撞）
    idle_inhibitor: Option<ZwpIdleInhibitorV1>,
    /// 申请把系统快捷键让给本窗口（通用窗口在 Linux 上用的是同一个协议）。
    /// 和通用窗口一样（flutter/linux/wayland_shortcuts_inhibit.cc）：拿到键盘焦点时申请、失去时撤掉，
    /// 这样用户按 Win+Esc 收回快捷键后，切走再切回来又让给远程（只申请一次的话 GNOME 不再自动恢复，2026-10-20 实测）。
    /// 合成器不支持时照常运行，系统快捷键留在本机
    shortcuts_manager: Option<ZwpKeyboardShortcutsInhibitManagerV1>,
    shortcuts_inhibitor: Option<ZwpKeyboardShortcutsInhibitorV1>,
    keyboard_seat: Option<wl_seat::WlSeat>,
    size: Option<(u32, u32)>,
    /// 收到过几次窗口配置：换全屏目标后等合成器的下一次配置用（两块屏一样大时尺寸不变，只能数次数）
    configures: u64,
    closed: bool,
    slots: Vec<Slot>,
    formats: Option<FormatSupport>,
    pending: HashMap<ObjectId, Commit>,
    events: Vec<Event>,
    import_failed: bool,
    seat: SeatState,
    shm: Shm,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// 长按连发的参数（每秒几次、按住多少毫秒后开始），合成器给的。合成器只发一次按下，连发要窗口自己生成：
    /// 远程 Windows 不会替注入的按键连发，RustDesk 通用窗口转发的也是本机系统连发出的一串按下。
    /// 每秒 0 次是合成器自己连发（wl_seat v10 的「重复」）或用户关了连发
    repeat_info: (u32, u32),
    /// 正在连发的键和下一次连发的时刻：只连发最后按下的那个键（和本机系统一样），松开它、失去焦点就停
    repeating: Option<(u32, Instant)>,
    /// 鼠标进入窗口时合成器给的序号，设置指针要带上它
    pointer_serial: Option<u32>,
    cursor_surface: wl_surface::WlSurface,
    cursor_pool: Option<SlotPool>,
    /// 当前的远程光标；None = 隐藏
    cursor: Option<CursorImage>,
    /// 窗口当前所在的屏（合成器报「进入」的最后一块）
    entered: Option<wl_output::WlOutput>,
    subcompositor: Option<SubcompositorState>,
    /// 叠加图层，按调用方给的编号；用到才建
    layers: Vec<Option<Layer>>,
}

impl State {
    /// 鼠标事件发生在哪：窗口本身，或某块叠加图层（换算回窗口坐标）。别的表面（光标）返回 None。
    fn window_pos(&self, surface: &wl_surface::WlSurface, (x, y): (f64, f64)) -> Option<(f64, f64)> {
        if surface == self.window.wl_surface() {
            return Some((x, y));
        }
        self.layers
            .iter()
            .flatten()
            .find(|l| &l.surface == surface)
            .map(|l| (x + l.pos.0 as f64, y + l.pos.1 as f64))
    }
}

/// 窗口所在的屏：显示链路检查用（找对应的显示接口、看画面有没有被缩放）
#[derive(Debug, Clone)]
pub struct OutputSummary {
    /// 合成器给的屏名（如 `DP-2`），不保证等于内核的显示接口名
    pub name: Option<String>,
    /// 当前分辨率（物理像素）
    pub mode: Option<(u32, u32)>,
    /// 在桌面坐标里的尺寸（逻辑像素）：和物理分辨率不一样就说明这块屏开了缩放
    pub logical: Option<(u32, u32)>,
}

/// 叠在画面上的一块小图层的内容（悬浮球、菜单、状态块）：窗口坐标里的位置，预乘透明度的 ARGB8888（内存里按 B G R A 排）。
pub struct LayerImage {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// 一块小图层：窗口表面的子表面，合成器把它叠在画面上面；画面本身照旧零拷贝，不动。
/// 子表面设成「不跟父表面同步」，内容一提交就生效；位置要等父表面下一次提交（画面每帧都提交）才生效。
struct Layer {
    surface: wl_surface::WlSurface,
    subsurface: WlSubsurface,
    pool: SlotPool,
    pos: (i32, i32),
}

pub struct Window {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
}

impl Window {
    /// 连上 Wayland、开一个全屏无边框窗口，等合成器给出尺寸和格式支持。`app_id` 是已安装的桌面文件名（不带 .desktop）。
    pub fn open(title: &str, app_id: &str) -> Result<Self> {
        let conn = Connection::connect_to_env().map_err(err("connect to the Wayland display"))?;
        let (globals, queue) = registry_queue_init::<State>(&conn).map_err(err("read the Wayland globals"))?;
        let qh = queue.handle();
        let compositor = CompositorState::bind(&globals, &qh).map_err(err("bind wl_compositor"))?;
        let xdg = XdgShell::bind(&globals, &qh).map_err(err("bind xdg_wm_base"))?;
        let dmabuf = DmabufState::new(&globals, &qh);
        if dmabuf.version().is_none() {
            return Err(Error("the compositor does not support linux-dmabuf".into()));
        }
        let presentation = PresentationTimeState::bind(&globals, &qh);
        let shm = Shm::bind(&globals, &qh).map_err(err("bind wl_shm"))?;
        let cursor_surface = compositor.create_surface(&qh);
        let surface = compositor.create_surface(&qh);
        let window = xdg.create_window(surface, WindowDecorations::None, &qh);
        window.set_title(title);
        // 和通用窗口同一个应用名（= 已安装的桌面文件名）：GNOME 只对认得出是哪个已安装应用的窗口记住
        // 「允许禁用快捷键」（RustDesk 设置页「清除快捷键授权」也按这个名字找），对不上时每开一个专业窗口（切显示器就会开新的）
        // 都要问一次
        window.set_app_id(app_id);
        window.set_fullscreen(None);
        window.commit();
        // 这个窗口的格式反馈（v4 起才有）：哪些格式能用、哪些能直接送显示硬件
        let feedback = dmabuf.get_surface_feedback(window.wl_surface(), &qh).ok();
        // 合成器不支持时照常运行，只是桌面可能按自己的设置黑屏
        let idle_inhibitor = globals
            .bind::<ZwpIdleInhibitManagerV1, _, _>(&qh, 1..=1, ())
            .ok()
            .map(|m| m.create_inhibitor(window.wl_surface(), &qh, ()));
        let shortcuts_manager = globals
            .bind::<ZwpKeyboardShortcutsInhibitManagerV1, _, _>(&qh, 1..=1, ())
            .ok();
        // 叠加图层用；合成器都支持，万一不支持就没有悬浮球，画面照常
        let subcompositor = SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh).ok();
        let mut w = Self {
            conn,
            queue,
            state: State {
                registry: RegistryState::new(&globals),
                outputs: OutputState::new(&globals, &qh),
                _compositor: compositor,
                _xdg: xdg,
                dmabuf,
                presentation,
                window,
                idle_inhibitor,
                shortcuts_manager,
                shortcuts_inhibitor: None,
                keyboard_seat: None,
                size: None,
                configures: 0,
                closed: false,
                slots: Vec::new(),
                formats: None,
                pending: HashMap::new(),
                events: Vec::new(),
                import_failed: false,
                seat: SeatState::new(&globals, &qh),
                shm,
                pointer: None,
                keyboard: None,
                repeat_info: (0, 0),
                repeating: None,
                pointer_serial: None,
                cursor_surface,
                cursor_pool: None,
                cursor: None,
                entered: None,
                subcompositor,
                layers: Vec::new(),
            },
        };
        while w.state.size.is_none() || (feedback.is_some() && w.state.formats.is_none()) {
            if w.state.closed {
                return Err(Error("the window was closed before it was shown".into()));
            }
            w.queue.blocking_dispatch(&mut w.state).map_err(err("wait for the window"))?;
        }
        w.state.events.clear();
        Ok(w)
    }

    pub fn size(&self) -> (u32, u32) {
        self.state.size.unwrap_or((0, 0))
    }

    /// 本机有几块屏
    pub fn screen_count(&self) -> usize {
        self.state.outputs.outputs().count()
    }

    /// 改成在本机第 `rank` 块屏上全屏（屏按桌面坐标从左到右、再从上到下排，从 0 数），等合成器按新屏配置好窗口；
    /// 返回那块屏的名字。要在第一次提交画面之前调：窗口还没上屏，看不到跳屏。没有这块屏时什么也不做，返回 None。
    pub fn fullscreen_on(&mut self, rank: usize) -> Result<Option<String>> {
        let s = &self.state;
        let mut screens: Vec<_> = s
            .outputs
            .outputs()
            .filter_map(|o| {
                let info = s.outputs.info(&o)?;
                let (x, y) = info.logical_position.unwrap_or(info.location);
                Some(((x, y), o, info.name.clone()))
            })
            .collect();
        screens.sort_by_key(|((x, y), ..)| (*x, *y));
        let Some((_, output, name)) = screens.into_iter().nth(rank) else {
            return Ok(None);
        };
        let seen = self.state.configures;
        self.state.window.set_fullscreen(Some(&output));
        self.state.window.commit();
        // 窗口本来就在那块屏上时合成器不一定再配置一次：最多等 1 秒
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.state.configures == seen && Instant::now() < deadline {
            if self.state.closed {
                return Err(Error("the window was closed before it was shown".into()));
            }
            self.dispatch(Duration::from_millis(50), None)?;
        }
        self.state.events.clear();
        Ok(Some(name.unwrap_or_default()))
    }

    /// 设置 / 撤掉第 `index` 块叠加图层。内容不变只挪位置时传同样的内容也行（照样重传一次，图层都很小）。
    /// 合成器不支持子表面时什么也不做。
    pub fn set_layer(&mut self, index: usize, image: Option<&LayerImage>) -> Result<()> {
        let qh = self.queue.handle();
        let s = &mut self.state;
        let Some(sub) = s.subcompositor.as_ref() else {
            return Ok(());
        };
        if s.layers.len() <= index {
            s.layers.resize_with(index + 1, || None);
        }
        let Some(img) = image else {
            if let Some(l) = &s.layers[index] {
                l.surface.attach(None, 0, 0);
                l.surface.commit();
                self.conn.flush().map_err(err("hide an overlay"))?;
            }
            return Ok(());
        };
        let (w, h) = (img.width as usize, img.height as usize);
        if w == 0 || h == 0 || img.bgra.len() != w * h * 4 {
            return Err(Error(format!("bad overlay image {w}x{h} with {} bytes", img.bgra.len())));
        }
        if s.layers[index].is_none() {
            let (subsurface, surface) = sub.create_subsurface(s.window.wl_surface().clone(), &qh);
            subsurface.set_desync();
            let pool = SlotPool::new(w * h * 4, &s.shm).map_err(err("create an overlay buffer pool"))?;
            s.layers[index] = Some(Layer {
                surface,
                subsurface,
                pool,
                pos: (i32::MIN, i32::MIN),
            });
        }
        let Some(l) = s.layers[index].as_mut() else {
            return Ok(());
        };
        let (buffer, canvas) = l
            .pool
            .create_buffer(w as i32, h as i32, (w * 4) as i32, wl_shm::Format::Argb8888)
            .map_err(err("create an overlay buffer"))?;
        // 池里的槽按对齐向上取整，可能比图像大（42×42 要 7056 字节、给了 7104）：只写图像那部分
        canvas
            .get_mut(..img.bgra.len())
            .ok_or_else(|| Error(format!("overlay buffer smaller than the image ({} bytes)", img.bgra.len())))?
            .copy_from_slice(&img.bgra);
        buffer.attach_to(&l.surface).map_err(err("attach an overlay buffer"))?;
        l.surface.damage_buffer(0, 0, w as i32, h as i32);
        l.surface.commit();
        if l.pos != (img.x, img.y) {
            l.pos = (img.x, img.y);
            l.subsurface.set_position(img.x, img.y);
            // 位置随父表面提交生效：画面停着不来新帧时也要马上挪，父表面不换内容再提交一次
            s.window.wl_surface().commit();
        }
        self.conn.flush().map_err(err("show an overlay"))?;
        Ok(())
    }

    /// 窗口所在的屏；合成器还没报「进入」时 None。
    pub fn output(&self) -> Option<OutputSummary> {
        let o = self.state.entered.as_ref()?;
        let info = self.state.outputs.info(o)?;
        Some(OutputSummary {
            name: info.name.clone(),
            mode: info
                .modes
                .iter()
                .find(|m| m.current)
                .map(|m| (m.dimensions.0.max(0) as u32, m.dimensions.1.max(0) as u32)),
            logical: info.logical_size.map(|(w, h)| (w.max(0) as u32, h.max(0) as u32)),
        })
    }

    /// 防黑屏是否生效（合成器支持 idle-inhibit）。
    pub fn idle_inhibited(&self) -> bool {
        self.state.idle_inhibitor.is_some()
    }

    /// 把远程光标设成本窗口的系统指针（由合成器画，和 RustDesk 界面的做法一样）；None 隐藏指针。
    pub fn set_cursor(&mut self, cursor: Option<CursorImage>) -> Result<()> {
        self.state.cursor = cursor;
        self.state.apply_cursor()?;
        self.conn.flush().map_err(err("send the cursor to the compositor"))
    }

    /// 合成器的格式反馈；合成器不支持 dmabuf v4 时为 None。
    pub fn formats(&self) -> Option<&FormatSupport> {
        self.state.formats.as_ref()
    }

    /// 准备 `count` 个槽位（丢掉以前导入的画面）。
    pub fn reset_slots(&mut self, count: usize) {
        for mut s in self.state.slots.drain(..) {
            s.clear();
        }
        self.state.slots = (0..count)
            .map(|_| Slot {
                buffer: None,
                params: None,
                busy: false,
            })
            .collect();
        // 旧提交的上屏回执带着旧槽号，晚到会被算到同槽号的新帧头上
        self.state.pending.clear();
    }

    /// 把一张导出的显存画面登记到第 `slot` 个槽位（每张只登记一次，之后反复提交）。
    pub fn import(&mut self, slot: usize, image: &Exported) -> Result<()> {
        let layer = image.layers.first().ok_or_else(|| Error("the exported image has no layer".into()))?;
        let modifier = image.modifiers.first().copied().unwrap_or(0);
        if let Some(f) = &self.state.formats {
            if !f.usable.contains(&(layer.drm_format, modifier)) {
                return Err(Error(format!(
                    "the compositor cannot use format {:#x} with modifier {modifier:#x}",
                    layer.drm_format
                )));
            }
        }
        let qh = self.queue.handle();
        let params = self.state.dmabuf.create_params(&qh).map_err(err("create dmabuf params"))?;
        for (i, p) in layer.planes.iter().enumerate() {
            let fd = image
                .fds
                .get(p.object as usize)
                .ok_or_else(|| Error("the exported image refers to a missing object".into()))?;
            params.add(fd.as_fd(), i as u32, p.offset, p.pitch, modifier);
        }
        let (buffer, params) =
            params.create_immed(image.width as i32, image.height as i32, layer.drm_format, Flags::empty(), &qh);
        self.conn.flush().map_err(err("send the dmabuf to the compositor"))?;
        let s = self
            .state
            .slots
            .get_mut(slot)
            .ok_or_else(|| Error(format!("no slot {slot}")))?;
        s.clear();
        s.buffer = Some(buffer);
        s.params = Some(params);
        s.busy = false;
        Ok(())
    }

    /// 一个合成器没拿着的槽位。
    pub fn free_slot(&self) -> Option<usize> {
        self.state.slots.iter().position(|s| s.buffer.is_some() && !s.busy)
    }

    /// 提交第 `slot` 个槽位的画面，并要上屏回执。
    pub fn present(&mut self, slot: usize) -> Result<()> {
        if self.state.import_failed {
            return Err(Error("the compositor rejected an imported dmabuf".into()));
        }
        let qh = self.queue.handle();
        let s = &self.state;
        let buffer = s
            .slots
            .get(slot)
            .and_then(|s| s.buffer.as_ref())
            .ok_or_else(|| Error(format!("slot {slot} has no image")))?;
        let surface = s.window.wl_surface();
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        let feedback = s.presentation.feedback(surface, &qh).ok();
        let commit_ns = now_ns();
        surface.commit();
        if let Some(fb) = feedback {
            self.state.pending.insert(fb.id(), Commit { slot, commit_ns });
        }
        self.state.slots[slot].busy = true;
        self.conn.flush().map_err(err("send the frame to the compositor"))?;
        Ok(())
    }

    /// 处理合成器发来的事件，没有就最多等 `timeout`。
    /// `wake`：调用方另一个要一起等的描述符（如「新画面到了」的通知），它可读时也立即返回，不让画面干等。
    pub fn dispatch(&mut self, timeout: Duration, wake: Option<BorrowedFd<'_>>) -> Result<Vec<Event>> {
        self.queue.dispatch_pending(&mut self.state).map_err(err("handle Wayland events"))?;
        if self.state.events.is_empty() {
            // 处理事件时排进去的请求（回合成器心跳的 pong、ack_configure、指针进来时设光标）先发出去再等：
            // 没有新画面要提交的时段（第一帧之前、一直解码出错）它们不发，合成器会判这个窗口「无响应」。
            // 库自带的 blocking_dispatch 也是先 flush 再等
            self.conn.flush().map_err(err("send requests to the compositor"))?;
            if let Some(guard) = self.queue.prepare_read() {
                let mut pfds = [
                    libc::pollfd {
                        fd: guard.connection_fd().as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: wake.map_or(-1, |w| w.as_raw_fd()),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                // 按住的键到连发时刻就醒来，不等画面
                let timeout = match self.state.repeating {
                    Some((_, at)) => timeout.min(at.saturating_duration_since(Instant::now())),
                    None => timeout,
                };
                let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
                let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, ms) };
                if n > 0 && pfds[0].revents != 0 {
                    match guard.read() {
                        Ok(_) => {}
                        // 别的读者先读走了（或被信号打断）：不是故障，下一轮再读
                        Err(WaylandError::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                        Err(e) => return Err(Error(format!("read Wayland events: {e}"))),
                    }
                }
            }
            self.queue.dispatch_pending(&mut self.state).map_err(err("handle Wayland events"))?;
        }
        if let Some((key, at)) = self.state.repeating {
            let now = Instant::now();
            if now >= at {
                // 一次只补一下：主循环被画面占着晚醒了也不一口气补一串
                let interval = Duration::from_millis(1000 / u64::from(self.state.repeat_info.0.max(1)));
                self.state.repeating = Some((key, now + interval));
                self.state.events.push(Event::Key { evdev: key, down: true });
            }
        }
        Ok(mem::take(&mut self.state.events))
    }
}

impl State {
    /// 失去键盘焦点：撤掉快捷键申请，系统快捷键回到本机（下次拿到焦点再申请）
    fn uninhibit_shortcuts(&mut self) {
        if let Some(i) = self.shortcuts_inhibitor.take() {
            i.destroy();
        }
    }

    /// 按当前光标设置指针；鼠标不在窗口里（没有进入序号）时等下次进入再设。
    fn apply_cursor(&mut self) -> Result<()> {
        let (Some(pointer), Some(serial)) = (self.pointer.clone(), self.pointer_serial) else {
            return Ok(());
        };
        let Some(c) = self.cursor.clone() else {
            pointer.set_cursor(serial, None, 0, 0);
            return Ok(());
        };
        let (w, h) = (c.width as usize, c.height as usize);
        if w == 0 || h == 0 || c.rgba.len() != w * h * 4 {
            return Err(Error(format!("bad cursor image {w}x{h} with {} bytes", c.rgba.len())));
        }
        let pool = match self.cursor_pool.as_mut() {
            Some(pool) => pool,
            None => self
                .cursor_pool
                .insert(SlotPool::new(w * h * 4, &self.shm).map_err(err("create the cursor buffer pool"))?),
        };
        let (buffer, canvas) = pool
            .create_buffer(w as i32, h as i32, (w * 4) as i32, wl_shm::Format::Argb8888)
            .map_err(err("create a cursor buffer"))?;
        // wl_shm 的 ARGB8888 在内存里按 B G R A 排，且要求预乘透明度
        for (dst, src) in canvas.chunks_exact_mut(4).zip(c.rgba.chunks_exact(4)) {
            let a = src[3] as u32;
            dst[0] = (src[2] as u32 * a / 255) as u8;
            dst[1] = (src[1] as u32 * a / 255) as u8;
            dst[2] = (src[0] as u32 * a / 255) as u8;
            dst[3] = src[3];
        }
        buffer.attach_to(&self.cursor_surface).map_err(err("attach the cursor buffer"))?;
        self.cursor_surface.damage_buffer(0, 0, w as i32, h as i32);
        self.cursor_surface.commit();
        pointer.set_cursor(serial, Some(&self.cursor_surface), c.hotx as i32, c.hoty as i32);
        Ok(())
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    // 光标也是一个表面，它进出屏不算
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, s: &wl_surface::WlSurface, o: &wl_output::WlOutput) {
        if s == self.window.wl_surface() {
            self.entered = Some(o.clone());
        }
    }
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, s: &wl_surface::WlSurface, o: &wl_output::WlOutput) {
        if s == self.window.wl_surface() && self.entered.as_ref() == Some(o) {
            self.entered = None;
        }
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for State {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &XdgWindow) {
        self.closed = true;
        self.events.push(Event::Closed);
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &XdgWindow, c: WindowConfigure, _: u32) {
        self.configures += 1;
        // 全屏时合成器会给出屏幕尺寸；万一没给，取第一块屏当前分辨率
        let size = match c.new_size {
            (Some(w), Some(h)) => Some((w.get(), h.get())),
            _ => self.outputs.outputs().find_map(|o| {
                let info = self.outputs.info(&o)?;
                let m = info.modes.iter().find(|m| m.current)?;
                Some((m.dimensions.0 as u32, m.dimensions.1 as u32))
            }),
        };
        if let Some((width, height)) = size {
            // 明说窗口就是画面这么大：不说的话合成器把叠加图层也算进窗口范围，贴顶边缩进的悬浮球（伸出屏幕上方）
            // 会让整个全屏窗口往下挪（2026-10-20 瘦客户机实撞）
            self.window.set_window_geometry(0, 0, width, height);
            if self.size != Some((width, height)) {
                self.size = Some((width, height));
                self.events.push(Event::Resized { width, height });
            }
        }
    }
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf
    }

    fn dmabuf_feedback(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        feedback: DmabufFeedback,
    ) {
        let table = feedback.format_table();
        let pick = |idx: &u16| table.get(*idx as usize).map(|f| (f.format, f.modifier));
        let mut support = FormatSupport::default();
        for t in feedback.tranches() {
            support.usable.extend(t.formats.iter().filter_map(pick));
            if matches!(t.flags, WEnum::Value(f) if f.contains(TrancheFlags::Scanout)) {
                support.scanout.extend(t.formats.iter().filter_map(pick));
            }
        }
        self.formats = Some(support);
    }

    fn created(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        _: wl_buffer::WlBuffer,
    ) {
    }

    fn failed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1) {
        self.import_failed = true;
    }

    fn released(&mut self, _: &Connection, _: &QueueHandle<Self>, buffer: &wl_buffer::WlBuffer) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.buffer.as_ref() == Some(buffer)) {
            s.busy = false;
        }
    }
}

impl PresentationTimeHandler for State {
    fn presentation_time_state(&mut self) -> &mut PresentationTimeState {
        &mut self.presentation
    }

    fn presented(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        feedback: &wp_presentation_feedback::WpPresentationFeedback,
        _: &wl_surface::WlSurface,
        _: Vec<wl_output::WlOutput>,
        time: PresentTime,
        _: u32,
        _: u64,
        flags: WEnum<Kind>,
    ) {
        let Some(c) = self.pending.remove(&feedback.id()) else { return };
        let kind = match flags {
            WEnum::Value(k) => k,
            WEnum::Unknown(_) => Kind::empty(),
        };
        self.events.push(Event::Presented {
            slot: c.slot,
            commit_ns: c.commit_ns,
            present_ns: time.tv_sec as i64 * 1_000_000_000 + time.tv_nsec as i64,
            zero_copy: kind.contains(Kind::ZeroCopy),
            foreign_clock: time.clk_id != libc::CLOCK_MONOTONIC as u32,
        });
    }

    fn discarded(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        feedback: &wp_presentation_feedback::WpPresentationFeedback,
        _: &wl_surface::WlSurface,
    ) {
        if let Some(c) = self.pending.remove(&feedback.id()) {
            self.events.push(Event::Discarded { slot: c.slot });
        }
    }
}

// 防黑屏的两个对象都没有事件
impl Dispatch<ZwpIdleInhibitManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpIdleInhibitManagerV1,
        _: zwp_idle_inhibit_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpIdleInhibitorV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpIdleInhibitorV1,
        _: zwp_idle_inhibitor_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpKeyboardShortcutsInhibitManagerV1,
        _: zwp_keyboard_shortcuts_inhibit_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitorV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpKeyboardShortcutsInhibitorV1,
        event: zwp_keyboard_shortcuts_inhibitor_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Active => state.events.push(Event::ShortcutsInhibited(true)),
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Inactive => state.events.push(Event::ShortcutsInhibited(false)),
            _ => {}
        }
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_compositor!(State);
delegate_output!(State);
delegate_xdg_shell!(State);
delegate_xdg_window!(State);
delegate_dmabuf!(State);
delegate_presentation_time!(State);
delegate_registry!(State);
delegate_seat!(State);
delegate_pointer!(State);
delegate_shm!(State);
delegate_subcompositor!(State);

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        match capability {
            Capability::Pointer if self.pointer.is_none() => {
                self.pointer = self.seat.get_pointer(qh, &seat).ok();
            }
            // 键盘不经 sctk 的 xkb 处理，直接收原始键码（RustDesk 的映射模式只要物理键码）
            Capability::Keyboard if self.keyboard.is_none() => {
                self.keyboard = Some(seat.get_keyboard(qh, ()));
                self.keyboard_seat = Some(seat);
            }
            _ => {}
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        match capability {
            Capability::Pointer => {
                if let Some(p) = self.pointer.take() {
                    p.release();
                }
                self.pointer_serial = None;
            }
            Capability::Keyboard => {
                if let Some(k) = self.keyboard.take() {
                    k.release();
                }
                self.keyboard_seat = None;
                self.repeating = None;
                self.uninhibit_shortcuts();
                self.events.push(Event::KeyboardLeave);
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for State {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for e in events {
            // 叠加图层上的事件也收：换算回窗口坐标，由调用方决定是交给图层（悬浮球）还是发给远程
            let Some((x, y)) = self.window_pos(&e.surface, e.position) else {
                continue;
            };
            match e.kind {
                PointerEventKind::Enter { serial } => {
                    self.pointer_serial = Some(serial);
                    // 指针设置出错只影响光标样子，不中断画面
                    self.apply_cursor().ok();
                    self.events.push(Event::PointerMotion { x, y });
                }
                PointerEventKind::Leave { .. } => {
                    self.pointer_serial = None;
                    self.events.push(Event::PointerLeave);
                }
                PointerEventKind::Motion { .. } => self.events.push(Event::PointerMotion { x, y }),
                PointerEventKind::Press { button, .. } => {
                    self.events.push(Event::PointerButton { button, down: true, x, y })
                }
                PointerEventKind::Release { button, .. } => {
                    self.events.push(Event::PointerButton { button, down: false, x, y })
                }
                PointerEventKind::Axis { horizontal, vertical, .. } => {
                    // 和 Flutter 窗口一样：每个滚动事件按方向算 1 格，不管幅度
                    let steps = |a: &sctk::seat::pointer::AxisScroll| {
                        if a.value120 != 0 {
                            a.value120.signum()
                        } else if a.discrete != 0 {
                            a.discrete.signum()
                        } else if a.absolute != 0.0 {
                            a.absolute.signum() as i32
                        } else {
                            0
                        }
                    };
                    let (dx, dy) = (steps(&horizontal), steps(&vertical));
                    if dx != 0 || dy != 0 {
                        self.events.push(Event::Wheel { dx, dy });
                    }
                }
            }
        }
    }
}

/// 不连发的键（和 xkb 默认布局一致）：左右 Ctrl / Shift / Alt / Win 和三个锁定键（evdev 键码）
fn is_modifier(evdev: u32) -> bool {
    matches!(evdev, 29 | 42 | 54 | 56 | 58 | 69 | 70 | 97 | 100 | 125 | 126)
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter { .. } => {
                if let (Some(m), Some(seat), None) =
                    (&state.shortcuts_manager, &state.keyboard_seat, &state.shortcuts_inhibitor)
                {
                    state.shortcuts_inhibitor = Some(m.inhibit_shortcuts(state.window.wl_surface(), seat, qh, ()));
                }
            }
            wl_keyboard::Event::Key { key, state: WEnum::Value(s), .. } => {
                let down = match s {
                    wl_keyboard::KeyState::Pressed => {
                        let (rate, delay) = state.repeat_info;
                        if rate > 0 && !is_modifier(key) {
                            state.repeating = Some((key, Instant::now() + Duration::from_millis(delay.into())));
                        }
                        true
                    }
                    wl_keyboard::KeyState::Released => {
                        if state.repeating.is_some_and(|(k, _)| k == key) {
                            state.repeating = None;
                        }
                        false
                    }
                    // 合成器自己连发（wl_seat v10）：和按下一样转发
                    wl_keyboard::KeyState::Repeated => true,
                    _ => return,
                };
                state.events.push(Event::Key { evdev: key, down });
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.repeat_info = (rate.max(0) as u32, delay.max(0) as u32);
                state.repeating = None;
            }
            wl_keyboard::Event::Modifiers { mods_depressed, mods_locked, .. } => state.events.push(Event::Modifiers {
                depressed: mods_depressed,
                locked: mods_locked,
            }),
            wl_keyboard::Event::Leave { .. } => {
                state.repeating = None;
                state.uninhibit_shortcuts();
                state.events.push(Event::KeyboardLeave);
            }
            // 键盘布局（xkb keymap）用不上：fd 随事件丢弃即关闭
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ 探测

/// 不开窗口，只看这台机器能不能做专业显示：在 Wayland 桌面里、合成器能用 dmabuf 的 XRGB8888 线性画面。
/// 不能时返回原因（谈判时如实报给被控端）。
pub fn probe() -> std::result::Result<(), String> {
    const XRGB8888: u32 = 0x3432_5258;
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("needs a Wayland desktop (cannot reach the Wayland display: {e})"))?;
    let (globals, mut queue) = registry_queue_init::<Probe>(&conn).map_err(|e| format!("read the Wayland globals: {e}"))?;
    let qh = queue.handle();
    let dmabuf = DmabufState::new(&globals, &qh);
    let Some(version) = dmabuf.version() else {
        return Err("the compositor does not support linux-dmabuf".into());
    };
    let feedback = dmabuf.get_default_feedback(&qh).ok();
    let mut p = Probe {
        registry: RegistryState::new(&globals),
        dmabuf,
        formats: None,
    };
    queue.roundtrip(&mut p).map_err(|e| format!("talk to the compositor: {e}"))?;
    if feedback.is_some() && p.formats.is_none() {
        queue.roundtrip(&mut p).map_err(|e| format!("talk to the compositor: {e}"))?;
    }
    // v4 起格式在反馈里，v3 及以下在 modifier 事件里
    let usable = match p.formats {
        Some(f) => f.contains(&(XRGB8888, 0)),
        None => p.dmabuf.modifiers().iter().any(|f| f.format == XRGB8888 && f.modifier == 0),
    };
    if !usable {
        return Err(format!("the compositor (linux-dmabuf v{version}) cannot use XRGB8888 linear frames"));
    }
    Ok(())
}

struct Probe {
    registry: RegistryState,
    dmabuf: DmabufState,
    formats: Option<Vec<(u32, u64)>>,
}

impl DmabufHandler for Probe {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf
    }

    fn dmabuf_feedback(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        feedback: DmabufFeedback,
    ) {
        let table = feedback.format_table();
        self.formats = Some(
            feedback
                .tranches()
                .iter()
                .flat_map(|t| t.formats.iter())
                .filter_map(|i| table.get(*i as usize).map(|f| (f.format, f.modifier)))
                .collect(),
        );
    }

    fn created(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        _: wl_buffer::WlBuffer,
    ) {
    }

    fn failed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1) {}

    fn released(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_buffer::WlBuffer) {}
}

impl ProvidesRegistryState for Probe {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![];
}

delegate_dmabuf!(Probe);
delegate_registry!(Probe);
