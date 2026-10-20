//! 主控端专业窗口进程（`--pro-view`）：会话进程拉起的子进程。
//!
//! 从标准输入收启动参数（画面通道）和远程光标；自己收专业画面、硬解、全屏显示；键鼠经标准输出交回会话进程，
//! 由会话进程走 RustDesk 原有的输入入口发给被控端。会话进程退出时标准输入断开，本进程随之退出。
//!
//! 三个线程：
//! - 收包：收画面、发心跳（兼授时、关键帧请求）。和解码显示分开——心跳停 3 秒被控端就退回普通画面，
//!   解码或显示偶尔慢一下不能被当成通道断了（第 5a 步长时间运行得出的约束）；
//! - 读标准输入：光标、远程这块屏的显示变化、会话进程退出；
//! - 主线程：窗口事件、解码、转 RGB、提交、键鼠。

use crate::ddpro::wire::PEER_LOG_INTERVAL;
use base::message_proto::{message, misc, CursorData, Message};
use ddpro::{
    dec::{fit_rect, Converter, Decoder, Device, RgbStandard},
    media::{Event as MediaEvent, Ledger, Receiver, Stage},
    view::{
        display_check::{Cards, Issue},
        now_ns, CursorImage, Event, Window,
    },
    view_down, view_ended::Kind, view_up, ViewDown, ViewEnded, ViewKey, ViewMouse, ViewStart,
    ViewStats, ViewUp,
};
use hbb_common::{anyhow::anyhow, bail, log, protobuf::Message as _, throttled_log, ResultType};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufReader, Read, Write},
    os::{
        fd::{AsFd, FromRawFd},
        unix::net::UnixStream,
    },
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Sender as Tx, TryRecvError},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const SLOTS: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REPORT: Duration = Duration::from_secs(5);
/// 质量面板多久报一次实测值（和 RustDesk 普通画面那条链路推面板数据的间隔一样）
const PANEL: Duration = Duration::from_secs(1);
/// 显示链路多久查一次（用户开关夜灯、改分辨率后几秒内面板就跟上）
const DISPLAY_CHECK: Duration = Duration::from_secs(5);

// 显示链路问题的代号，质量面板按它显示名字和处理办法（`flutter/lib/common/widgets/ddpro_quality.dart`）
const ISSUE_LIMITED: &str = "limited_range";
const ISSUE_NIGHT_LIGHT: &str = "night_light";
const ISSUE_GAMMA: &str = "gamma_curve";
const ISSUE_TRANSFORM: &str = "color_transform";
const ISSUE_SCALED: &str = "scaled";
/// 没有窗口事件、也没有新画面时最多等这么久（只为定时检查会话是否还在；新画面由收包线程叫醒，不靠它）
const IDLE_WAIT: Duration = Duration::from_millis(100);

// RustDesk 鼠标入口的编码（`src/common.rs`）：mask = 事件类型 | (按键 << 3)
const MOUSE_TYPE_MOVE: i32 = 0;
const MOUSE_TYPE_DOWN: i32 = 1;
const MOUSE_TYPE_UP: i32 = 2;
const MOUSE_TYPE_WHEEL: i32 = 3;

// XKB 的标准修饰位（Wayland 键盘事件里的掩码）
const XKB_SHIFT: u32 = 1 << 0;
const XKB_LOCK: u32 = 1 << 1;
const XKB_CONTROL: u32 = 1 << 2;
const XKB_MOD1_ALT: u32 = 1 << 3;
const XKB_MOD2_NUM: u32 = 1 << 4;
const XKB_MOD4_SUPER: u32 = 1 << 6;

/// 收包线程交给主线程的一帧：码流、各段时间（本机单调时钟，纳秒）。
struct Incoming {
    data: Vec<u8>,
    /// 被控端拿到画面的时刻、首包到达、末包到达；时钟还没对上时 None
    times: Option<Times>,
}

#[derive(Clone, Copy)]
struct Times {
    captured: i64,
    stages: [(Stage, i64); 4],
    last_arrival: i64,
}

/// 读标准输入的线程交给主线程的
enum FromSession {
    Cursor(CursorImage),
    /// 远程这块屏的新位置和尺寸（被控端切了分辨率等）
    Display(i32, i32, i32, i32),
}

/// 收包线程的累计统计，给主线程报质量面板用（收包线程每收一轮照抄一遍）
#[derive(Default)]
struct Counters {
    bytes: AtomicU64,
    lost: AtomicU64,
    recovered: AtomicU64,
}

/// 质量面板这一秒的实测值
struct Panel {
    since: Instant,
    frames: u32,
    latency_us: Vec<i64>,
    last_bytes: u64,
    /// 实际解出的画面：色度是否 4:4:4、位深
    format: Option<(bool, u32)>,
    /// 实际解出的画面尺寸
    frame: Option<(u32, u32)>,
    /// 上次报的「是否缩放」（变了才写日志）
    scaled: Option<bool>,
}

impl Panel {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            frames: 0,
            latency_us: Vec::new(),
            last_bytes: 0,
            format: None,
            frame: None,
            scaled: None,
        }
    }

    /// 显示链路这一秒的结论：检查线程读到的显示属性，加上画面是否逐像素（解出的尺寸 = 窗口尺寸，屏没开缩放）。
    fn display(&mut self, win: &Window, watch: &Mutex<DisplayWatch>) -> (bool, Vec<String>) {
        let out = win.output();
        let (checked, mut issues) = {
            let mut w = watch.lock().unwrap();
            w.want = out.as_ref().and_then(|o| o.name.clone());
            match &w.result {
                Some(Ok(v)) => (true, v.iter().map(|s| s.to_string()).collect()),
                _ => (false, Vec::new()),
            }
        };
        let window = win.size();
        let screen_scaled = out
            .as_ref()
            .is_some_and(|o| o.logical.is_some() && o.mode.is_some() && o.logical != o.mode);
        let scaled = self.frame.is_some_and(|f| f != window) || screen_scaled;
        if self.scaled != Some(scaled) && self.frame.is_some() {
            self.scaled = Some(scaled);
            log::info!(
                "ddpro: pro view scaled: {scaled} (frame {:?}, window {window:?}, screen {:?})",
                self.frame,
                out.map(|o| (o.name, o.mode, o.logical))
            );
        }
        if scaled {
            issues.push(ISSUE_SCALED.to_owned());
        }
        (checked, issues)
    }

    /// 取走这一秒的值并清零
    fn take(&mut self, counters: &Counters, (display_checked, display_issues): (bool, Vec<String>)) -> ViewStats {
        let secs = self.since.elapsed().as_secs_f64().max(0.001);
        self.since = Instant::now();
        let bytes = counters.bytes.load(Ordering::Relaxed);
        let kbps = (bytes.saturating_sub(self.last_bytes) as f64 * 8.0 / 1000.0 / secs).round() as u32;
        self.last_bytes = bytes;
        self.latency_us.sort_unstable();
        let latency_ms = self
            .latency_us
            .get(self.latency_us.len() / 2)
            .map_or(-1, |us| ((us + 500) / 1000) as i32);
        self.latency_us.clear();
        let fps = (self.frames as f64 / secs).round() as u32;
        self.frames = 0;
        let (chroma, bit_depth) = match self.format {
            Some((true, depth)) => ("4:4:4", depth),
            Some((false, depth)) => ("not 4:4:4", depth),
            None => ("", 0),
        };
        ViewStats {
            fps,
            kbps,
            latency_ms,
            chroma: chroma.to_owned(),
            bit_depth,
            lost_packets: counters.lost.load(Ordering::Relaxed),
            recovered_packets: counters.recovered.load(Ordering::Relaxed),
            display_checked,
            display_issues,
            ..Default::default()
        }
    }
}

/// 主线程和显示链路检查线程之间：主线程写窗口所在屏的名字，检查线程写结论
#[derive(Default)]
struct DisplayWatch {
    want: Option<String>,
    /// 查到的问题（多语言表的键）；读不了显示属性时是原因
    result: Option<Result<Vec<&'static str>, String>>,
}

/// 显示链路检查线程：每隔一会儿读一次显示属性，结论变了写日志。伽马曲线不是原样时再问 GNOME 夜灯开没开，好写明原因。
fn watch_display(watch: Arc<Mutex<DisplayWatch>>, quit: Arc<AtomicBool>) {
    // 窗口已经开出来，合成器正占着显示主控权：这时打开显卡，之后一直用这一份（见 `Cards`）
    let cards = Cards::open();
    let mut last = String::new();
    while !quit.load(Ordering::Acquire) {
        let want = watch.lock().unwrap().want.clone();
        let result = cards.as_ref().map_err(String::clone).and_then(|c| c.check(want.as_deref())).map(|outputs| {
            let mut issues = Vec::new();
            for i in outputs.iter().flat_map(|o| &o.issues) {
                let text = match i {
                    Issue::LimitedRange => ISSUE_LIMITED,
                    Issue::GammaCurve if night_light_active() == Some(true) => ISSUE_NIGHT_LIGHT,
                    Issue::GammaCurve => ISSUE_GAMMA,
                    Issue::ColorTransform => ISSUE_TRANSFORM,
                };
                if !issues.contains(&text) {
                    issues.push(text);
                }
            }
            (issues, outputs)
        });
        // 结论变了才写日志；读到的数值（如伽马曲线偏差）附在那一行里，但不参与比较——夜灯日落时慢慢变色，
        // 数值会一直变
        let (key, detail) = match &result {
            Ok((issues, outputs)) => (
                format!(
                    "{issues:?}; {}",
                    outputs
                        .iter()
                        .map(|o| format!("{}: {:?}", o.connector, o.issues))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
                outputs
                    .iter()
                    .map(|o| format!("{}: {}", o.connector, o.detail))
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            Err(e) => (format!("cannot read the display output settings: {e}"), String::new()),
        };
        if key != last {
            log::info!("ddpro: pro view display output (window on {want:?}): {key} ({detail})");
            last = key;
        }
        watch.lock().unwrap().result = Some(result.map(|(issues, _)| issues));
        let until = Instant::now() + DISPLAY_CHECK;
        while Instant::now() < until && !quit.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// GNOME 的夜灯此刻是否在起作用（会话总线上设置守护进程的属性）；问不到时 None。
fn night_light_active() -> Option<bool> {
    use dbus::blocking::{stdintf::org_freedesktop_dbus::Properties, Connection};
    let c = Connection::new_session().ok()?;
    c.with_proxy(
        "org.gnome.SettingsDaemon.Color",
        "/org/gnome/SettingsDaemon/Color",
        Duration::from_millis(500),
    )
    .get("org.gnome.SettingsDaemon.Color", "NightLightActive")
    .ok()
}

/// 已提交、等上屏回执的一帧
struct Shown {
    times: Times,
    decoded: i64,
    converted: i64,
    committed: i64,
}

pub fn run() {
    // 标准输出拿来回传消息：先把它另存一份当专用通道，再把原来的标准输出指到标准错误，
    // 这样任何库往标准输出打的字都不会混进消息里
    // 指不过去就不能用：库打到标准输出的字会混进消息里，干净地失败（会话进程按「专业窗口意外退出」处理）
    let mut up = unsafe {
        let fd = hbb_common::libc::dup(1);
        if fd < 0 || hbb_common::libc::dup2(2, 1) < 0 {
            log::error!(
                "ddpro: pro view cannot set up its channel to the session: {}",
                std::io::Error::last_os_error()
            );
            return;
        }
        File::from_raw_fd(fd)
    };
    let (kind, detail) = match run_(&mut up) {
        Ok((kind, detail)) => {
            log::info!("ddpro: pro view ended: {detail}");
            (kind, detail)
        }
        Err(e) => {
            log::error!("ddpro: pro view failed: {e}");
            (Kind::DISPLAY_ERROR, e.to_string())
        }
    };
    let ended = ViewEnded {
        kind: kind.into(),
        detail,
        ..Default::default()
    };
    send_up(&mut up, view_up::Union::Ended(ended)).ok();
}

fn send_up(up: &mut File, union: view_up::Union) -> ResultType<()> {
    let msg = ViewUp {
        union: Some(union),
        ..Default::default()
    };
    ddpro::write_frame(up, &msg)?;
    Ok(())
}

/// 收包线程记下的画面通道情况：通过了没有、为什么结束（专业窗口结束时据此说清是「没通」还是「通了又断」）
#[derive(Default)]
struct ChannelEnd {
    opened: bool,
    why: String,
}

fn run_(up: &mut File) -> ResultType<(Kind, String)> {
    let mut stdin = BufReader::new(std::io::stdin());
    let start: ViewStart = match ddpro::read_frame::<ViewDown>(&mut stdin)? {
        Some(ViewDown {
            union: Some(view_down::Union::Start(s)),
            ..
        }) => s,
        _ => bail!("the session did not send the start parameters"),
    };
    let (peer, keys) = ddpro::media_endpoint(&start.media)
        .ok_or_else(|| anyhow!("unusable pro channel parameters"))?;

    // 先连通道：被控端同意后只等 3 秒打招呼，开窗口、初始化显卡要花时间，不能让它等。
    // 这期间到的画面在队列里等着（第一帧总是关键帧）。收包线程每收到一帧往 `wake` 写一个字节叫醒主线程
    let quit = Arc::new(AtomicBool::new(false));
    let bye = Arc::new(AtomicBool::new(false));
    // 主线程解码出错时置上，收包线程看到就让画面通道重新同步（等关键帧、请求关键帧）
    let resync = Arc::new(AtomicBool::new(false));
    let channel = Arc::new(Mutex::new(ChannelEnd::default()));
    let counters = Arc::new(Counters::default());
    let (frame_tx, frame_rx) = mpsc::channel();
    let (wake_rx, wake_tx) = UnixStream::pair()?;
    wake_rx.set_nonblocking(true)?;
    wake_tx.set_nonblocking(true)?;
    let receiver = {
        let (quit, bye, resync, channel, counters) =
            (quit.clone(), bye.clone(), resync.clone(), channel.clone(), counters.clone());
        std::thread::Builder::new()
            .name("ddpro-view-recv".into())
            .spawn(move || receive(peer, keys, frame_tx, wake_tx, quit, bye, resync, channel, counters))?
    };
    let (mut win, dev, mut dec, mut conv) = match open_display(&start) {
        Ok(v) => v,
        Err(e) => {
            // 显示不了：马上告诉被控端不要了（不让它白等 3 秒心跳超时），它退回普通画面
            bye.store(true, Ordering::Release);
            quit.store(true, Ordering::Release);
            receiver.join().ok();
            return Err(e);
        }
    };
    let mut size = win.size();
    let display_watch = Arc::new(Mutex::new(DisplayWatch::default()));
    {
        let (watch, quit) = (display_watch.clone(), quit.clone());
        std::thread::Builder::new()
            .name("ddpro-view-display".into())
            .spawn(move || watch_display(watch, quit))?;
    }
    let (session_tx, session_rx) = mpsc::channel();
    let session_gone = Arc::new(AtomicBool::new(false));
    {
        let gone = session_gone.clone();
        std::thread::Builder::new()
            .name("ddpro-view-stdin".into())
            .spawn(move || read_session(stdin, session_tx, gone))?;
    }

    let mut input = Input::new(&start, size);
    let mut ledger = Ledger::default();
    let mut shown: HashMap<usize, Shown> = HashMap::new();
    let mut last_report = Instant::now();
    let mut panel = Panel::new();
    let mut ready = false;
    // 查过实际解码格式的画面尺寸：第一帧和尺寸变了之后（被控端重建编码器）查一次
    let mut format_checked: Option<(u32, u32)> = None;
    // 主循环包一层：中途任何一步出错，也马上告诉被控端不要了（不让它白等 3 秒心跳超时）
    let mut main_loop = || -> ResultType<(Kind, String)> {
        Ok(loop {
            if session_gone.load(Ordering::Acquire) {
                // 会话结束或会话进程要换屏：告诉被控端不要了，它马上结束这一路（不等 3 秒心跳超时）
                bye.store(true, Ordering::Release);
                break (Kind::SESSION_CLOSED, "the session closed the pro view".to_owned());
            }
            for e in win
                .dispatch(IDLE_WAIT, Some(wake_rx.as_fd()))
                .map_err(|e| anyhow!("{e}"))?
            {
                match e {
                    Event::Closed => {
                        bye.store(true, Ordering::Release);
                        quit.store(true, Ordering::Release);
                    }
                    Event::Presented {
                        slot,
                        present_ns,
                        foreign_clock,
                        ..
                    } => {
                        if let (Some(s), false) = (shown.remove(&slot), foreign_clock) {
                            let mut stages = s.times.stages.to_vec();
                            stages.extend([
                                (Stage::Decode, (s.decoded - s.times.last_arrival) / 1000),
                                (Stage::Convert, (s.converted - s.decoded) / 1000),
                                (Stage::Submit, (s.committed - s.converted) / 1000),
                                (Stage::Display, (present_ns - s.committed) / 1000),
                            ]);
                            ledger.add_frame(&stages, (present_ns - s.times.captured) / 1000);
                            panel.latency_us.push((present_ns - s.times.captured) / 1000);
                        }
                    }
                    Event::Discarded { slot } => {
                        shown.remove(&slot);
                    }
                    // 本机屏幕分辨率变了：按新尺寸重建转换输出（窗口只在尺寸变了时报；尺寸相同就不动，防万一重复报）
                    Event::Resized { width, height } if (width, height) != size => {
                        log::info!(
                            "ddpro: pro view resized from {}x{} to {width}x{height}",
                            size.0,
                            size.1
                        );
                        size = (width, height);
                        shown.clear();
                        conv = attach(&mut win, &dev)?;
                        input.set_window(size);
                    }
                    Event::Resized { .. } => {}
                    other => {
                        for up_msg in input.handle(other) {
                            if send_up(up, up_msg).is_err() {
                                session_gone.store(true, Ordering::Release);
                            }
                        }
                    }
                }
            }
            if quit.load(Ordering::Acquire) {
                if bye.load(Ordering::Acquire) {
                    break (Kind::USER_CLOSED, "the user closed the pro view".to_owned());
                }
                let c = channel.lock().unwrap();
                break if c.opened {
                    (Kind::CHANNEL_LOST, format!("the pro channel ended: {}", c.why))
                } else {
                    (Kind::CHANNEL_FAILED, format!("the pro channel never opened: {}", c.why))
                };
            }
            loop {
                match session_rx.try_recv() {
                    Ok(FromSession::Cursor(c)) => {
                        if let Err(e) = win.set_cursor(Some(c)) {
                            throttled_log!(
                                PEER_LOG_INTERVAL,
                                warn,
                                "ddpro: pro view cannot set the cursor: {e}"
                            );
                        }
                    }
                    Ok(FromSession::Display(x, y, w, h)) => {
                        // 多久来一次由被控端决定，节流
                        throttled_log!(
                            PEER_LOG_INTERVAL,
                            info,
                            "ddpro: pro view: the remote display is now {w}x{h} at ({x},{y})"
                        );
                        input.set_remote(x, y, w, h);
                    }
                    Err(_) => break,
                }
            }
            // 先清掉叫醒字节，再取画面：之后到的画面会再写字节，不会漏
            let mut sink = [0u8; 64];
            while (&wake_rx).read(&mut sink).is_ok_and(|n| n > 0) {}
            // 每一帧都要解（后面的帧参考前面的），只把最新解出的那一帧拿去显示
            let mut latest = None;
            loop {
                match frame_rx.try_recv() {
                    Ok(f) => {
                        let f: Incoming = f;
                        match dec.decode(&f.data) {
                            Ok(Some(frame)) => latest = Some((frame, f.times)),
                            Ok(None) => {}
                            // 码流坏了或驱动报错：参考帧不可信了。编码端是无限 GOP，不要关键帧就一直坏下去
                            Err(e) => {
                                throttled_log!(
                                    PEER_LOG_INTERVAL,
                                    warn,
                                    "ddpro: pro view decode error, asking for a keyframe: {e}"
                                );
                                resync.store(true, Ordering::Release);
                            }
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        quit.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            if let Some((frame, times)) = latest {
                // 门槛按结果定（P0 设计决定 3）：实际解出来的不是谈好的 4:4:4 8bit 就大声失败，不静默降级
                let size = (frame.width(), frame.height());
                if format_checked != Some(size) {
                    let f = frame.format();
                    if !(f.chroma_444 && f.bit_depth == 8) {
                        bail!("the decoded video is {} ({}bit, 4:4:4 {}), not the agreed 4:4:4 8bit", f.name, f.bit_depth, f.chroma_444);
                    }
                    format_checked = Some(size);
                    panel.format = Some((f.chroma_444, f.bit_depth));
                    panel.frame = Some(size);
                }
                // 显卡驱动的单帧解码错误常常在这一步才报出来（iHD：等表面时报解码错误）：和上面一样丢这一帧、
                // 要关键帧，不当成专业窗口的致命错误
                if let Err(e) = dec.wait(&frame) {
                    throttled_log!(
                        PEER_LOG_INTERVAL,
                        warn,
                        "ddpro: pro view decode error while waiting for the GPU, asking for a keyframe: {e}"
                    );
                    resync.store(true, Ordering::Release);
                    continue;
                }
                let decoded = now_ns();
                let Some(slot) = win.free_slot() else {
                    throttled_log!(
                        PEER_LOG_INTERVAL,
                        info,
                        "ddpro: pro view dropped a frame: the compositor holds every image"
                    );
                    continue;
                };
                conv.convert(&frame, slot, RgbStandard::Bt709)
                    .map_err(|e| anyhow!("{e}"))?;
                conv.wait(slot).map_err(|e| anyhow!("{e}"))?;
                let converted = now_ns();
                win.present(slot).map_err(|e| anyhow!("{e}"))?;
                panel.frames += 1;
                if !ready {
                    // 第一帧专业画面交给合成器了：会话进程据此撤掉「非专业模式」提示
                    ready = true;
                    if send_up(up, view_up::Union::Ready(true)).is_err() {
                        session_gone.store(true, Ordering::Release);
                    }
                }
                if let Some(times) = times {
                    shown.insert(
                        slot,
                        Shown {
                            times,
                            decoded,
                            converted,
                            committed: now_ns(),
                        },
                    );
                }
            }
            if last_report.elapsed() >= REPORT {
                last_report = Instant::now();
                log::info!("ddpro: pro view latency {}", ledger.take());
            }
            if panel.since.elapsed() >= PANEL {
                let display = panel.display(&win, &display_watch);
                if send_up(up, view_up::Union::Stats(panel.take(&counters, display))).is_err() {
                    session_gone.store(true, Ordering::Release);
                }
            }
        })
    };
    let result = main_loop();
    if result.is_err() {
        bye.store(true, Ordering::Release);
    }
    quit.store(true, Ordering::Release);
    receiver.join().ok();
    result
}

/// 开全屏窗口、显卡解码器和转换器（设备留着，本机屏幕分辨率变了要按新尺寸重建转换器）。
fn open_display(start: &ViewStart) -> ResultType<(Window, Device, Decoder, Converter)> {
    let mut win = Window::open(if start.title.is_empty() {
        "dreamdesk-pro"
    } else {
        &start.title
    })
    .map_err(|e| anyhow!("{e}"))?;
    let (w, h) = win.size();
    let dev = Device::open().map_err(|e| anyhow!("{e}"))?;
    let dec = Decoder::new(&dev).map_err(|e| anyhow!("{e}"))?;
    let conv = attach(&mut win, &dev)?;
    log::info!(
        "ddpro: pro view {w}x{h} fullscreen for remote {}x{} at ({},{}), keeping the screen awake: {}",
        start.remote_width,
        start.remote_height,
        start.remote_x,
        start.remote_y,
        win.idle_inhibited()
    );
    Ok((win, dev, dec, conv))
}

/// 按窗口当前尺寸建转换器，把它的几张输出画面登记给窗口（换掉原来登记的）。
fn attach(win: &mut Window, dev: &Device) -> ResultType<Converter> {
    let (w, h) = win.size();
    let conv = Converter::new(dev, w, h, SLOTS).map_err(|e| anyhow!("{e}"))?;
    win.reset_slots(SLOTS);
    for slot in 0..SLOTS {
        let image = conv.export(slot).map_err(|e| anyhow!("{e}"))?;
        win.import(slot, &image).map_err(|e| anyhow!("{e}"))?;
    }
    Ok(conv)
}

/// 收包线程：收画面交给主线程（并往 `wake` 写一个字节叫醒它）；主线程要退出时，用户关窗的话先告诉被控端不要了。
fn receive(
    peer: std::net::SocketAddr,
    keys: ddpro::media::Keys,
    frames: Tx<Incoming>,
    wake: UnixStream,
    quit: Arc<AtomicBool>,
    bye: Arc<AtomicBool>,
    resync: Arc<AtomicBool>,
    channel: Arc<Mutex<ChannelEnd>>,
    counters: Arc<Counters>,
) {
    let mut r = match Receiver::connect(peer, keys, CONNECT_TIMEOUT, 0) {
        Ok(r) => r,
        Err(e) => {
            log::error!("ddpro: pro view cannot open the pro channel to {peer}: {e}");
            channel.lock().unwrap().why = e.to_string();
            quit.store(true, Ordering::Release);
            return;
        }
    };
    log::info!("ddpro: pro view channel open from {peer}");
    channel.lock().unwrap().opened = true;
    let mut last_stats = Instant::now();
    let end = loop {
        if resync.swap(false, Ordering::AcqRel) {
            r.resync();
        }
        if quit.load(Ordering::Acquire) {
            if bye.load(Ordering::Acquire) {
                if let Err(e) = r.bye() {
                    log::warn!("ddpro: pro view cannot tell the peer it is closing: {e}");
                }
            }
            break "closing".to_owned();
        }
        match r.recv() {
            Ok(MediaEvent::Frame { data, times, .. }) => {
                // 收包时刻在收包线程自己的时间轴上，换成本机单调时钟交给主线程
                let shift = now_ns() - r.now_us() * 1000;
                let times = times.and_then(|t| {
                    Some(Times {
                        captured: t.captured_at()? * 1000 + shift,
                        stages: t.transport_stages()?,
                        last_arrival: t.last_arrival * 1000 + shift,
                    })
                });
                if frames.send(Incoming { data, times }).is_err() {
                    break "the window is gone".to_owned();
                }
                // 缓冲满（写不进）说明主线程已经有一个没读的叫醒，不用再写
                (&wake).write(&[1]).ok();
            }
            Ok(MediaEvent::Loss { packets }) => throttled_log!(
                PEER_LOG_INTERVAL,
                info,
                "ddpro: pro view lost {packets} packet(s), keyframe requested"
            ),
            Err(e) => break e.to_string(),
        }
        counters.bytes.store(r.stats.bytes, Ordering::Relaxed);
        counters.lost.store(r.stats.lost_packets, Ordering::Relaxed);
        counters.recovered.store(r.stats.recovered, Ordering::Relaxed);
        if last_stats.elapsed() >= REPORT {
            last_stats = Instant::now();
            let clock = r.clock().map_or("no clock sync yet".to_owned(), |c| {
                format!("clock rtt {:.2} ms", c.rtt_us as f64 / 1000.0)
            });
            log::info!("ddpro: pro view channel {:?}; {clock}", r.stats);
        }
    };
    log::info!(
        "ddpro: pro view channel closed ({end}); totals {:?}",
        r.stats
    );
    channel.lock().unwrap().why = end;
    quit.store(true, Ordering::Release);
}

/// 读标准输入：远程光标、远程这块屏的显示变化；会话进程退出（管道断开）时置 `gone`。
fn read_session(
    mut stdin: BufReader<std::io::Stdin>,
    to_main: Tx<FromSession>,
    gone: Arc<AtomicBool>,
) {
    let mut cache: HashMap<u64, CursorImage> = HashMap::new();
    loop {
        let msg = match ddpro::read_frame::<ViewDown>(&mut stdin) {
            Ok(Some(m)) => m,
            Ok(None) => break,
            Err(e) => {
                log::warn!("ddpro: pro view lost the session channel: {e}");
                break;
            }
        };
        let Some(view_down::Union::Rustdesk(bytes)) = msg.union else {
            continue;
        };
        let m = match Message::parse_from_bytes(&bytes) {
            Ok(m) => m,
            Err(e) => {
                throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: pro view got a malformed message from the session: {e}");
                continue;
            }
        };
        let out = match m.union {
            Some(message::Union::CursorData(cd)) => {
                let id = cd.id;
                match cursor_image(cd) {
                    Ok(c) => {
                        cache.insert(id, c.clone());
                        FromSession::Cursor(c)
                    }
                    Err(e) => {
                        throttled_log!(
                            PEER_LOG_INTERVAL,
                            warn,
                            "ddpro: pro view rejected cursor {id}: {e}"
                        );
                        continue;
                    }
                }
            }
            Some(message::Union::CursorId(id)) => match cache.get(&id) {
                Some(c) => FromSession::Cursor(c.clone()),
                None => continue,
            },
            Some(message::Union::Misc(misc)) => match misc.union {
                Some(misc::Union::SwitchDisplay(s)) => FromSession::Display(s.x, s.y, s.width, s.height),
                _ => continue,
            },
            _ => continue,
        };
        if to_main.send(out).is_err() {
            break;
        }
    }
    gone.store(true, Ordering::Release);
}

/// RustDesk 光标数据 → 图案：解压和校验用 RustDesk 自己的（和 Flutter 界面拿到的完全一样）。
fn cursor_image(cd: CursorData) -> ResultType<CursorImage> {
    let cd = crate::client::io_loop::kept_cursor_rgba(cd)?;
    Ok(CursorImage {
        width: cd.width as u32,
        height: cd.height as u32,
        hotx: cd.hotx as u32,
        hoty: cd.hoty as u32,
        rgba: cd.colors.to_vec(),
    })
}

/// 窗口里的键鼠 → 交给会话进程的消息（和 Flutter 窗口交给 RustDesk 的值一致）。
struct Input {
    /// 本机窗口尺寸
    window: (u32, u32),
    /// 远程这块屏在远程桌面坐标里的位置和尺寸
    remote: (i32, i32, i32, i32),
    /// 远程画面在窗口里的位置和大小
    fit: (f64, f64, f64, f64),
    depressed: u32,
    locked: u32,
    /// 按着的键（evdev），焦点离开时替用户抬起
    held: HashSet<u32>,
    pos: (i32, i32),
}

impl Input {
    fn new(start: &ViewStart, window: (u32, u32)) -> Self {
        let mut input = Self {
            window,
            remote: (0, 0, 1, 1),
            fit: (0.0, 0.0, 1.0, 1.0),
            depressed: 0,
            locked: 0,
            held: HashSet::new(),
            pos: (start.remote_x, start.remote_y),
        };
        input.set_remote(
            start.remote_x,
            start.remote_y,
            start.remote_width as i32,
            start.remote_height as i32,
        );
        input
    }

    fn set_remote(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.remote = (x, y, w.max(1), h.max(1));
        self.refit();
    }

    fn set_window(&mut self, window: (u32, u32)) {
        self.window = window;
        self.refit();
    }

    /// 和转换器同一个算法（等比缩放、居中），坐标换算才和画面对得上
    fn refit(&mut self) {
        let (_, _, rw, rh) = self.remote;
        let (x, y, fw, fh) = fit_rect(rw as u32, rh as u32, self.window.0, self.window.1);
        self.fit = (x as f64, y as f64, fw.max(1) as f64, fh.max(1) as f64);
    }

    /// 窗口坐标 → 远程桌面坐标（去掉黑边、按缩放比例，夹在这块屏以内）
    fn remote_pos(&self, x: f64, y: f64) -> (i32, i32) {
        let (fx, fy, fw, fh) = self.fit;
        let (rx, ry, rw, rh) = self.remote;
        let px = ((x - fx) * rw as f64 / fw).floor() as i32;
        let py = ((y - fy) * rh as f64 / fh).floor() as i32;
        (rx + px.clamp(0, rw - 1), ry + py.clamp(0, rh - 1))
    }

    fn lock_modes(&self) -> i32 {
        let mut m = 0;
        if self.locked & XKB_LOCK != 0 {
            m |= 1 << 1;
        }
        if self.locked & XKB_MOD2_NUM != 0 {
            m |= 1 << 2;
        }
        m
    }

    fn mouse(&self, mask: i32, (x, y): (i32, i32)) -> view_up::Union {
        view_up::Union::Mouse(ViewMouse {
            mask,
            x,
            y,
            alt: self.depressed & XKB_MOD1_ALT != 0,
            ctrl: self.depressed & XKB_CONTROL != 0,
            shift: self.depressed & XKB_SHIFT != 0,
            command: self.depressed & XKB_MOD4_SUPER != 0,
            ..Default::default()
        })
    }

    fn key(&self, evdev: u32, down: bool) -> view_up::Union {
        view_up::Union::Key(ViewKey {
            // X 键码 = evdev + 8，和 Flutter 在 Linux 上给的位置码相同
            position_code: evdev as i32 + 8,
            down,
            lock_modes: self.lock_modes(),
            ..Default::default()
        })
    }

    fn handle(&mut self, e: Event) -> Vec<view_up::Union> {
        match e {
            Event::Modifiers { depressed, locked } => {
                self.depressed = depressed;
                self.locked = locked;
                vec![]
            }
            Event::Key { evdev, down } => {
                if down {
                    self.held.insert(evdev);
                } else {
                    self.held.remove(&evdev);
                }
                vec![self.key(evdev, down)]
            }
            Event::KeyboardLeave => {
                let held: Vec<u32> = self.held.drain().collect();
                held.into_iter().map(|k| self.key(k, false)).collect()
            }
            Event::PointerMotion { x, y } => {
                self.pos = self.remote_pos(x, y);
                vec![self.mouse(MOUSE_TYPE_MOVE, self.pos)]
            }
            Event::PointerButton { button, down, x, y } => {
                // Linux BTN_LEFT / RIGHT / MIDDLE / SIDE / EXTRA → RustDesk 左 / 右 / 中 / 后退 / 前进
                let b = match button {
                    0x110 => 0x01,
                    0x111 => 0x02,
                    0x112 => 0x04,
                    0x113 => 0x08,
                    0x114 => 0x10,
                    _ => return vec![],
                };
                self.pos = self.remote_pos(x, y);
                let t = if down { MOUSE_TYPE_DOWN } else { MOUSE_TYPE_UP };
                vec![self.mouse(t | (b << 3), self.pos)]
            }
            // Wayland 往下 / 往右为正，Flutter 窗口发的是反号（往下滚 -1）
            Event::Wheel { dx, dy } => vec![self.mouse(MOUSE_TYPE_WHEEL, (-dx, -dy))],
            _ => vec![],
        }
    }
}
