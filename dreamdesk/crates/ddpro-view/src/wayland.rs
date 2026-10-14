//! 全屏 Wayland 窗口：导入显存画面（dmabuf）、提交、收「已释放」和「已上屏」回执。
//!
//! 调用方自己驱动循环：`present` 提交一张画面，`dispatch` 等事件（带超时）。画面轮流用一组槽位，
//! 合成器还拿着的槽位（没回「已释放」）不能再写，[`Window::free_slot`] 只给空闲的。

use ddpro_dec::Exported;
use sctk::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_dmabuf, delegate_output, delegate_presentation_time, delegate_registry,
    delegate_xdg_shell, delegate_xdg_window,
    dmabuf::{DmabufFeedback, DmabufHandler, DmabufState},
    output::{OutputHandler, OutputState},
    presentation_time::{PresentTime, PresentationTimeHandler, PresentationTimeState},
    reexports::{
        client::{
            backend::ObjectId,
            globals::registry_queue_init,
            protocol::{wl_buffer, wl_output, wl_surface},
            Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
        },
        protocols::wp::{
            idle_inhibit::zv1::client::{
                zwp_idle_inhibit_manager_v1::{self, ZwpIdleInhibitManagerV1},
                zwp_idle_inhibitor_v1::{self, ZwpIdleInhibitorV1},
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
    shell::{
        xdg::{
            window::{Window as XdgWindow, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
};
use std::{collections::HashMap, fmt, mem, os::fd::AsFd, os::fd::AsRawFd, time::Duration};

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
        refresh_ns: u32,
        zero_copy: bool,
        vsync: bool,
        /// 合成器报的时钟不是本机单调时钟，`present_ns` 不能和 `commit_ns` 直接相减
        foreign_clock: bool,
    },
    /// 这一帧被合成器跳过了（下一帧来得太快，或窗口不可见）
    Discarded { slot: usize },
    Resized { width: u32, height: u32 },
    Closed,
}

/// 合成器对这个窗口说的格式支持：能用的（格式, modifier），以及其中能直接送显示硬件的。
#[derive(Debug, Default, Clone)]
pub struct FormatSupport {
    pub usable: Vec<(u32, u64)>,
    pub scanout: Vec<(u32, u64)>,
}

struct Slot {
    buffer: Option<wl_buffer::WlBuffer>,
    busy: bool,
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
    size: Option<(u32, u32)>,
    closed: bool,
    slots: Vec<Slot>,
    formats: Option<FormatSupport>,
    pending: HashMap<ObjectId, Commit>,
    events: Vec<Event>,
    import_failed: bool,
}

pub struct Window {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
}

impl Window {
    /// 连上 Wayland、开一个全屏无边框窗口，等合成器给出尺寸和格式支持。
    pub fn open(title: &str) -> Result<Self> {
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
        let surface = compositor.create_surface(&qh);
        let window = xdg.create_window(surface, WindowDecorations::None, &qh);
        window.set_title(title);
        window.set_app_id("dreamdesk-pro-view");
        window.set_fullscreen(None);
        window.commit();
        // 这个窗口的格式反馈（v4 起才有）：哪些格式能用、哪些能直接送显示硬件
        let feedback = dmabuf.get_surface_feedback(window.wl_surface(), &qh).ok();
        // 合成器不支持时照常运行，只是桌面可能按自己的设置黑屏
        let idle_inhibitor = globals
            .bind::<ZwpIdleInhibitManagerV1, _, _>(&qh, 1..=1, ())
            .ok()
            .map(|m| m.create_inhibitor(window.wl_surface(), &qh, ()));
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
                size: None,
                closed: false,
                slots: Vec::new(),
                formats: None,
                pending: HashMap::new(),
                events: Vec::new(),
                import_failed: false,
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

    /// 防黑屏是否生效（合成器支持 idle-inhibit）。
    pub fn idle_inhibited(&self) -> bool {
        self.state.idle_inhibitor.is_some()
    }

    /// 合成器的格式反馈；合成器不支持 dmabuf v4 时为 None。
    pub fn formats(&self) -> Option<&FormatSupport> {
        self.state.formats.as_ref()
    }

    /// 准备 `count` 个槽位（丢掉以前导入的画面）。
    pub fn reset_slots(&mut self, count: usize) {
        for s in self.state.slots.drain(..) {
            if let Some(b) = s.buffer {
                b.destroy();
            }
        }
        self.state.slots = (0..count).map(|_| Slot { buffer: None, busy: false }).collect();
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
        let (buffer, _) =
            params.create_immed(image.width as i32, image.height as i32, layer.drm_format, Flags::empty(), &qh);
        self.conn.flush().map_err(err("send the dmabuf to the compositor"))?;
        let s = self
            .state
            .slots
            .get_mut(slot)
            .ok_or_else(|| Error(format!("no slot {slot}")))?;
        if let Some(old) = s.buffer.replace(buffer) {
            old.destroy();
        }
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
    pub fn dispatch(&mut self, timeout: Duration) -> Result<Vec<Event>> {
        self.queue.dispatch_pending(&mut self.state).map_err(err("handle Wayland events"))?;
        if self.state.events.is_empty() {
            if let Some(guard) = self.queue.prepare_read() {
                let mut pfd = libc::pollfd {
                    fd: guard.connection_fd().as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
                let n = unsafe { libc::poll(&mut pfd, 1, ms) };
                if n > 0 {
                    guard.read().map_err(err("read Wayland events"))?;
                }
            }
            self.queue.dispatch_pending(&mut self.state).map_err(err("handle Wayland events"))?;
        }
        Ok(mem::take(&mut self.state.events))
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
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
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
        refresh: u32,
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
            refresh_ns: refresh,
            zero_copy: kind.contains(Kind::ZeroCopy),
            vsync: kind.contains(Kind::Vsync),
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
