//! 主控端专业窗口进程（`--pro-view`）：会话进程拉起的子进程。
//!
//! 从标准输入收启动参数（画面通道）和远程光标；自己收专业画面、硬解、全屏显示；键鼠经标准输出交回会话进程，
//! 由会话进程走 RustDesk 原有的输入入口发给被控端。会话进程退出时标准输入断开，本进程随之退出。
//!
//! 线程：
//! - 收包：收画面、发心跳（兼授时、关键帧请求）。和解码显示分开——心跳停 3 秒被控端就退回通用画面，
//!   解码或显示偶尔慢一下不能被当成通道断了（第 5a 步长时间运行得出的约束）；
//! - 读标准输入：光标、远程这块屏的显示变化、会话进程退出；
//! - 显示链路检查：隔一会儿读一次窗口所在屏的显示属性，结论变了写日志；
//! - 无损方块解压（`view_lossless`）；
//! - 主线程：窗口事件、解码、转 RGB、提交、键鼠。

use crate::ddpro::{
    ball::{Action, Ball},
    view_lossless::{self, Decoders},
    wire::PEER_LOG_INTERVAL,
};
use base::message_proto::{message, misc, CursorData, Message};
use ddpro::{
    dec::{fit_rect, Converter, Decoder, Device, GlConverter, RgbStandard},
    media::{Event as MediaEvent, Ledger, Receiver, Stage},
    view::{
        display_check::{Cards, Issue},
        now_ns, premultiplied_bgra, CursorImage, Event, LayerImage, Window,
    },
    view_action, view_down, view_ended::Kind, view_up, ViewAction, ViewDown, ViewEnded, ViewKey, ViewMouse,
    ViewResolution, ViewStart, ViewUp,
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
/// DRM 格式 XRGB8888（'XR24'）
const XRGB8888: u32 = 0x3432_5258;
/// DRM_FORMAT_XRGB2101010（'XR30'）：HDR 原样输出的 10 位画面（P1 第 4 项）
const XRGB2101010: u32 = 0x3033_5258;
/// 输出画面想要的分块排布（按偏好）：Intel 的 Y 分块、X 分块。带压缩辅助平面的排布不要——着色器导入只认单平面
const TILED: [u64; 2] = [0x0100_0000_0000_0002, 0x0100_0000_0000_0001];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REPORT: Duration = Duration::from_secs(5);
/// 收包线程没有事件时多久回来看一次退出 / 重新同步标志：远程静止时每秒才来一帧，只靠事件醒来的话关窗、要关键帧都要晚近 1 秒
const RECV_POLL: Duration = Duration::from_millis(100);
/// 悬浮球网络状态块多久更新一次（和 RustDesk 通用画面那条链路推质量面板数据的间隔一样）
const PANEL: Duration = Duration::from_secs(1);
/// 显示链路多久查一次（用户开关夜灯、改分辨率后几秒内日志就跟上）
const DISPLAY_CHECK: Duration = Duration::from_secs(5);

// 显示链路问题的代号（写进日志）
const ISSUE_LIMITED: &str = "limited_range";
const ISSUE_NIGHT_LIGHT: &str = "night_light";
const ISSUE_GAMMA: &str = "gamma_curve";
const ISSUE_TRANSFORM: &str = "color_transform";
/// 没有窗口事件、也没有新画面时最多等这么久（只为定时检查会话是否还在；新画面由收包线程叫醒，不靠它）
const IDLE_WAIT: Duration = Duration::from_millis(100);
/// 诊断：画面每次补到整屏无损时，从着色器那侧读回刚画好的画面（RGB 逐像素、从上到下）存到这个文件，和被控端原图逐像素比。
/// 文件名后面接 `.屏号`（多屏时每块屏一个专业窗口，各存各的）
const DIAG_LOSSLESS_DUMP: &str = "DDPRO_DIAG_LOSSLESS_DUMP";

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
    /// RTP 时间戳和被控端说这一帧哪些方块变了（静止无损）
    ts: u32,
    changed: Option<Vec<u8>>,
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
    /// 被控端的光标被别的东西挪到了这里（远程桌面坐标），见 [`RemoteCursor`]
    CursorPos(i32, i32),
    /// 远程这块屏的新位置和尺寸（被控端切了分辨率等），和它支持的分辨率、原始分辨率（悬浮球「分辨率」菜单用；
    /// 消息里没带列表时为空）
    Display {
        rect: (i32, i32, i32, i32),
        resolutions: Vec<(u32, u32)>,
        original: (u32, u32),
    },
}

/// 收包线程的累计统计，给主线程更新悬浮球状态块用（收包线程每收一轮照抄一遍）
#[derive(Default)]
struct Counters {
    bytes: AtomicU64,
    lost: AtomicU64,
    recovered: AtomicU64,
    /// 专业通道对时顺带量出的往返时间（微秒），还没对上时为 0（悬浮球的网络状态块用）
    rtt_us: AtomicU64,
}

/// 专业画面这一秒的实测值（悬浮球网络状态块显示）。色度、位深是从本机实际解出的画面读的，不是谈判时被控端声称的
struct Stats {
    /// 这一秒交给合成器上屏的帧数
    fps: u32,
    /// 这一秒收到的码率（千比特每秒）
    kbps: u32,
    /// 端到端延迟中位数：被控端拿到画面 → 本机上屏，毫秒；两端时钟还没对上、或这一秒没有可算的帧时为 -1
    latency_ms: i32,
    /// 还没解出第一帧时为空、位深为 0
    chroma: &'static str,
    bit_depth: u32,
    /// 从专业窗口打开起累计：救不回来的丢包、缺了又补齐的包
    lost_packets: u64,
    recovered_packets: u64,
}

/// 攒这一秒的实测值
struct Panel {
    since: Instant,
    frames: u32,
    latency_us: Vec<i64>,
    last_bytes: u64,
    /// 实际解出的画面：色度是否 4:4:4、位深
    format: Option<(bool, u32)>,
    /// 实际解出的画面尺寸。也是查实际解码格式的时机：第一帧和尺寸变了之后（被控端重建编码器）查一次
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

    /// 告诉显示链路检查线程窗口现在在哪块屏上；画面是否逐像素（解出的尺寸 = 窗口尺寸，屏没开缩放）变了就写日志。
    fn display(&mut self, win: &Window, want: &Mutex<Option<String>>) {
        let out = win.output();
        *want.lock().unwrap() = out.as_ref().and_then(|o| o.name.clone());
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
    }

    /// 取走这一秒的值并清零
    fn take(&mut self, counters: &Counters) -> Stats {
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
        Stats {
            fps,
            kbps,
            latency_ms,
            chroma,
            bit_depth,
            lost_packets: counters.lost.load(Ordering::Relaxed),
            recovered_packets: counters.recovered.load(Ordering::Relaxed),
        }
    }
}

/// 显示链路检查线程：每隔一会儿读一次显示属性（窗口所在的屏，主线程写进 `want`），结论变了写日志。
/// 伽马曲线不是原样时再问 GNOME 夜灯开没开，好写明原因。
fn watch_display(want: Arc<Mutex<Option<String>>>, quit: Arc<AtomicBool>) {
    // 窗口已经开出来，合成器正占着显示主控权：这时打开显卡，之后一直用这一份（见 `Cards`）
    let cards = Cards::open();
    let mut last = String::new();
    while !quit.load(Ordering::Acquire) {
        let want = want.lock().unwrap().clone();
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

/// 悬浮球网络状态块的几行：帧率、码率、专业通道往返、端到端延迟、丢包与补齐、画面尺寸、实际解出的格式。
/// 画面静止时被控端只重编上一帧、不带采集时间，端到端延迟这一秒算不出，显示「-」。
/// `lossless` 是画面里无损方块的比例（1:1 着色器显示时才有）
fn status_lines(v: &Stats, rtt_us: u64, frame: Option<(u32, u32)>, lossless: Option<u32>) -> Vec<String> {
    let mut lines = vec![
        format!("{} fps", v.fps),
        format!("{:.1} Mbps", v.kbps as f64 / 1000.0),
        if rtt_us > 0 { format!("RTT {:.1} ms", rtt_us as f64 / 1000.0) } else { "RTT -".to_owned() },
        if v.latency_ms >= 0 { format!("e2e {} ms", v.latency_ms) } else { "e2e -".to_owned() },
        format!("lost {} rec {}", v.lost_packets, v.recovered_packets),
    ];
    if let Some((w, h)) = frame {
        lines.push(format!("{w}x{h}"));
    }
    if v.bit_depth > 0 {
        lines.push(format!("H265 {} {}bit", v.chroma, v.bit_depth));
    }
    if let Some(p) = lossless {
        lines.push(format!("lossless {p}%"));
    }
    lines
}

/// 交给会话进程的菜单动作；「断开连接」不经会话进程（专业窗口自己按用户关窗结束），返回 None
fn view_action(a: Action) -> Option<ViewAction> {
    let union = match a {
        Action::SwitchDisplay(i) => view_action::Union::SwitchDisplay(i),
        Action::CtrlAltDel => view_action::Union::CtrlAltDel(true),
        Action::LockScreen => view_action::Union::LockScreen(true),
        Action::ShowStatus(on) => view_action::Union::ShowStatus(on),
        Action::Mic(on) => view_action::Union::Mic(on),
        Action::Hdr(on) => view_action::Union::Hdr(on),
        Action::GeneralMode => view_action::Union::GeneralMode(true),
        Action::ChangeResolution(width, height) => view_action::Union::ChangeResolution(ViewResolution {
            width,
            height,
            ..Default::default()
        }),
        Action::Disconnect => return None,
    };
    Some(ViewAction {
        union: Some(union),
        ..Default::default()
    })
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
    let (decoders, mut lossless) = view_lossless::start(&wake_tx)?;
    let receiver = {
        let (quit, bye, resync, channel, counters) =
            (quit.clone(), bye.clone(), resync.clone(), channel.clone(), counters.clone());
        std::thread::Builder::new()
            .name("ddpro-view-recv".into())
            .spawn(move || receive(peer, keys, frame_tx, wake_tx, decoders, quit, bye, resync, channel, counters))?
    };
    let (mut win, dev, mut dec, mut conv, mut shader, display_fixed) = match open_display(&start) {
        Ok(v) => v,
        Err(e) => {
            // 显示不了：马上告诉被控端不要了（不让它白等 3 秒心跳超时），它退回通用画面
            bye.store(true, Ordering::Release);
            quit.store(true, Ordering::Release);
            receiver.join().ok();
            return Err(e);
        }
    };
    let mut size = win.size();
    let display_watch = Arc::new(Mutex::new(None));
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

    let mut input = Input::new(&start, size, display_fixed);
    let mut remote_cursor = RemoteCursor::default();
    let mut ball = Ball::new(
        size,
        start.display_origins.len() as u32,
        start.display_index,
        display_fixed,
        start.show_status,
        start.mic,
        start.format.hdr,
        start.hdr_capable && super::hdr_client::available(),
    );
    let mut ledger = Ledger::default();
    let mut shown: HashMap<usize, Shown> = HashMap::new();
    let mut last_report = Instant::now();
    // 这一段日志间隔里：(合成器直接送显示的帧数, 上屏的帧数)
    let mut scanout = (0u64, 0u64);
    let mut panel = Panel::new();
    let mut ready = false;
    // 谈成的位深（老版本被控端的答复里可能没有，按 8bit），HDR 时被控端普通内容的白（P1 第 4 项）
    let want_bits = start.format.bit_depth.max(8);
    let hdr_white = start.format.hdr.then_some(start.format.sdr_white_nits);
    shader = hdr_shader(shader, hdr_white);
    let lossless_dump = std::env::var_os(DIAG_LOSSLESS_DUMP).map(|p| {
        let mut p = p;
        p.push(format!(".{}", start.display_index));
        p
    });
    let mut was_full = false;
    // 诊断（同上开关）：最近一次带变化的画面到达的时刻，量「无损补齐后多久上屏」
    let mut last_change = Instant::now();
    // 这一段日志间隔里因为静止而没上屏的帧数
    let mut idle_frames = 0u64;
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
                        zero_copy,
                        ..
                    } => {
                        scanout.0 += zero_copy as u64;
                        scanout.1 += 1;
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
                        // 着色器先放掉（窗口还登记着它的画面，下面重新登记），再按新尺寸建；视频处理单元的旧转换器
                        // 等新的建好、赋值时才放，中间短暂多占一套输出画面
                        let old = shader.take();
                        let on = old.is_some();
                        // HDR 原样输出的照旧先试 10 位画面（建不起来时 attach 撤掉 HDR 标记、退回普通画面）
                        let hdr = old.as_ref().is_some_and(|s| s.format() == XRGB2101010);
                        drop(old);
                        (conv, shader) = attach(&mut win, &dev, on, hdr)?;
                        shader = hdr_shader(shader.take(), hdr_white);
                        // 无损纹理随旧转换器没了：要关键帧，被控端整屏重新补
                        lossless.reset();
                        resync.store(true, Ordering::Release);
                        input.set_window(size);
                        ball.set_screen(size);
                    }
                    Event::Resized { .. } => {}
                    Event::PointerLeave => {
                        ball.pointer_leave();
                        for up_msg in input.handle(Event::PointerLeave) {
                            if send_up(up, up_msg).is_err() {
                                session_gone.store(true, Ordering::Release);
                            }
                        }
                    }
                    // 只在用户允许 / 拒绝、窗口得失键盘焦点时来，不会刷屏
                    Event::ShortcutsInhibited(on) => log::info!(
                        "ddpro: pro view system shortcuts {}",
                        if on { "go to the remote" } else { "stay with this computer" }
                    ),
                    other => {
                        if matches!(other, Event::PointerMotion { .. }) {
                            if let Err(e) = remote_cursor.local_moved(&mut win) {
                                throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: pro view cannot restore the pointer: {e}");
                            }
                        }
                        // 鼠标、按键先给悬浮球：落在球 / 菜单上的不发给远程
                        let (mine, action) = match &other {
                            Event::PointerMotion { x, y } => (ball.pointer_motion(*x, *y), None),
                            Event::PointerButton { button, down, x, y } => ball.pointer_button(*button, *down, *x, *y),
                            Event::Wheel { .. } => (ball.wheel(), None),
                            // 已经交给远程的键（按下时菜单还没开），它的连发和松开也给远程，不让悬浮球截走一半
                            Event::Key { evdev, down } if !input.held.contains(evdev) => (ball.key(*evdev, *down), None),
                            Event::KeyboardLeave => {
                                ball.keyboard_leave();
                                (false, None)
                            }
                            _ => (false, None),
                        };
                        let mut ups = if mine { vec![] } else { input.handle(other) };
                        match action {
                            // 菜单里的「断开连接」：和用户关掉专业窗口一样，结束这次远程
                            Some(Action::Disconnect) => {
                                bye.store(true, Ordering::Release);
                                quit.store(true, Ordering::Release);
                            }
                            Some(a) => ups.extend(view_action(a).map(view_up::Union::Action)),
                            None => {}
                        }
                        for up_msg in ups {
                            if send_up(up, up_msg).is_err() {
                                session_gone.store(true, Ordering::Release);
                            }
                        }
                    }
                }
            }
            ball.tick(Instant::now());
            for (index, image) in ball.render() {
                // 叠加图层出错只影响悬浮球，不中断画面
                if let Err(e) = win.set_layer(index, image.as_ref()) {
                    throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: pro view cannot show the floating menu: {e}");
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
                        if let Err(e) = remote_cursor.set_image(&mut win, c) {
                            throttled_log!(
                                PEER_LOG_INTERVAL,
                                warn,
                                "ddpro: pro view cannot set the cursor: {e}"
                            );
                        }
                    }
                    Ok(FromSession::CursorPos(x, y)) => {
                        if let Err(e) = remote_cursor.moved(&mut win, &input, x, y) {
                            throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: pro view cannot draw the remote cursor: {e}");
                        }
                    }
                    Ok(FromSession::Display {
                        rect: (x, y, w, h),
                        resolutions,
                        original,
                    }) => {
                        // 多久来一次由被控端决定，节流
                        throttled_log!(
                            PEER_LOG_INTERVAL,
                            info,
                            "ddpro: pro view: the remote display is now {w}x{h} at ({x},{y})"
                        );
                        input.set_remote(x, y, w, h);
                        ball.set_resolutions((w.max(0) as u32, h.max(0) as u32), resolutions, original);
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
                        if lossless_dump.is_some() && f.changed.as_ref().is_some_and(|b| b.iter().any(|&x| x != 0)) {
                            last_change = Instant::now();
                        }
                        lossless.changed(f.ts, f.changed.as_deref());
                        match dec.decode(&f.data) {
                            Ok(Some(frame)) => {
                                lossless.resize(frame.width(), frame.height(), f.ts);
                                latest = Some((frame, f.times));
                            }
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
            // 已到的帧都记上了变化，再放方块（见 view_lossless 开头）；只在 1:1 着色器显示时用
            let one_to_one = lossless.size() == Some(size);
            lossless.apply(shader.as_mut().filter(|_| one_to_one));
            // 静止且整屏已是无损像素：屏幕上的就是被控端的原图，这一帧解出来再画一遍也一字不差——只等解码完（解码出错照样要关键帧），
            // 不转换、不交给合成器（静止时专业窗口和合成器每秒白画 60 帧）。哪块一动，那一帧照常画
            let latest = match latest {
                Some((frame, _)) if one_to_one && shader.is_some() && lossless.settled() => {
                    wait_decoded(&dec, &frame, &resync);
                    idle_frames += 1;
                    None
                }
                other => other,
            };
            if let Some((frame, times)) = latest {
                // 门槛按结果定（P0 设计决定 3）：实际解出来的不是谈好的 4:4:4、谈好的位深就大声失败，不静默降级
                // 不叫 size：下面「1:1 才用着色器」要和外层的窗口尺寸比
                let frame_size = (frame.width(), frame.height());
                if panel.frame != Some(frame_size) {
                    let f = frame.format();
                    if !(f.chroma_444 && f.bit_depth == want_bits) {
                        bail!(
                            "the decoded video is {} ({}bit, 4:4:4 {}), not the agreed 4:4:4 {want_bits}bit",
                            f.name,
                            f.bit_depth,
                            f.chroma_444
                        );
                    }
                    panel.format = Some((f.chroma_444, f.bit_depth));
                    panel.frame = Some(frame_size);
                    // 画面尺寸变了，解码器重建了画面池，编号会被新画面复用：着色器按编号缓存的导入作废
                    if let Some(s) = shader.as_mut() {
                        s.forget_sources();
                    }
                }
                if !wait_decoded(&dec, &frame, &resync) {
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
                // 1:1 用着色器转（和公式逐像素一致）；要缩放、或着色器出过错，用视频处理单元
                let by_shader = match shader.as_mut() {
                    Some(s) if (frame.width(), frame.height()) == size => {
                        lossless.update_mask(s);
                        match s.convert(&dec, &frame, slot) {
                            Ok(()) => true,
                            Err(e) => {
                                log::warn!("ddpro: pro view shader conversion failed, using the video processor from now on: {e}");
                                shader = None;
                                false
                            }
                        }
                    }
                    _ => false,
                };
                if let (Some(path), Some(s), true) = (&lossless_dump, shader.as_ref(), by_shader) {
                    let full = lossless.coverage() == Some(100);
                    if full && !was_full {
                        match s.read_rgb(slot).map_err(|e| e.to_string()).and_then(|rgb| std::fs::write(path, rgb).map_err(|e| e.to_string())) {
                            Ok(()) => log::info!(
                                "ddpro: diagnostic {DIAG_LOSSLESS_DUMP}: saved a fully lossless frame, shown {} ms after the last change arrived",
                                last_change.elapsed().as_millis()
                            ),
                            Err(e) => log::warn!("ddpro: diagnostic {DIAG_LOSSLESS_DUMP}: {e}"),
                        }
                    }
                    was_full = full;
                }
                if !by_shader {
                    // 视频处理单元只按 BT.709 转，做不了 PQ 还原：HDR 画面要缩放（被控端分辨率和本机屏不一样）时大声失败，不显示成发白的颜色
                    if hdr_white.is_some() {
                        if (frame.width(), frame.height()) != size {
                            bail!(
                                "HDR video is shown only 1:1 ({}x{} video on a {}x{} screen): set the remote resolution to match this screen",
                                frame.width(),
                                frame.height(),
                                size.0,
                                size.1
                            );
                        }
                        // 尺寸对得上、却没走着色器：着色器设不上 HDR 或出过错（原因在上面的告警里），不是分辨率的事
                        bail!("HDR video needs the shader conversion, which failed on this computer (see the warning before)");
                    }
                    conv.convert(&frame, slot, RgbStandard::Bt709)
                        .map_err(|e| anyhow!("{e}"))?;
                    conv.wait(slot).map_err(|e| anyhow!("{e}"))?;
                }
                let converted = now_ns();
                win.present(slot).map_err(|e| anyhow!("{e}"))?;
                lossless.presented(by_shader);
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
                // 合成器直接送显示（不再合成一遍）的帧数：叠加层、输出画面的排布都会影响它
                log::info!(
                    "ddpro: pro view latency {}; direct scanout {}/{} presented, {} not presented (still); lossless {:?}% {:?}",
                    ledger.take(),
                    scanout.0,
                    scanout.1,
                    idle_frames,
                    lossless.coverage(),
                    lossless.counts
                );
                scanout = (0, 0);
                idle_frames = 0;
                lossless.counts = Default::default();
            }
            if panel.since.elapsed() >= PANEL {
                panel.display(&win, &display_watch);
                let stats = panel.take(&counters);
                let coverage = lossless.coverage().filter(|_| shader.is_some() && lossless.size() == Some(size));
                ball.set_status(status_lines(&stats, counters.rtt_us.load(Ordering::Relaxed), panel.frame, coverage));
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
/// 最后一项：多屏时这个窗口固定看这块远程屏（见 [`screen_rank`]）。
fn open_display(start: &ViewStart) -> ResultType<(Window, Device, Decoder, Converter, Option<GlConverter>, bool)> {
    // 桌面文件按应用名装（产品版 dreamdesk-pro.desktop，原版 rustdesk.desktop），和通用窗口同一个应用名
    let app_id = crate::get_app_name().to_lowercase();
    let mut win = Window::open(
        if start.title.is_empty() { &app_id } else { &start.title },
        &app_id,
    )
    .map_err(|e| anyhow!("{e}"))?;
    let rank = screen_rank(start, win.screen_count());
    if let Some(rank) = rank {
        // 还没送过画面，窗口没上屏：这时换全屏目标看不到跳屏
        let screen = win.fullscreen_on(rank).map_err(|e| anyhow!("{e}"))?;
        log::info!(
            "ddpro: pro view of remote display {} goes full screen on local screen {} ({screen:?}) of {}",
            start.display_index,
            rank + 1,
            win.screen_count()
        );
    }
    let (w, h) = win.size();
    let dev = Device::open().map_err(|e| anyhow!("{e}"))?;
    let dec = Decoder::new(&dev).map_err(|e| anyhow!("{e}"))?;
    let hdr_out = start.format.hdr && hdr_output(&mut win, start.format.sdr_white_nits);
    let (conv, shader) = attach(&mut win, &dev, true, hdr_out)?;
    log::info!(
        "ddpro: pro view {w}x{h} fullscreen for remote {}x{} at ({},{}), keeping the screen awake: {}",
        start.remote_width,
        start.remote_height,
        start.remote_x,
        start.remote_y,
        win.idle_inhibited()
    );
    Ok((win, dev, dec, conv, shader, rank.is_some()))
}

/// 多屏（P1 第 8 项，主控端打开了「将我的所有显示器用于远程会话」）：本机和远程都多于一块屏时，这块远程屏的窗口全屏到
/// 按位置对应的本机屏——两边都从左到右、再从上到下排，远程第 k 块对本机第 k 块，鼠标跨屏的方向和实物摆放一致。
/// RustDesk 按屏号给前 min(远程, 本机) 块远程屏开窗口，所以只在这几块里排。返回本机屏的排位；不是多屏时 None
/// （照旧由桌面决定放哪块屏）。
fn screen_rank(start: &ViewStart, local_screens: usize) -> Option<usize> {
    if !start.all_displays || local_screens < 2 || start.display_origins.len() < 2 {
        return None;
    }
    let shown = start.display_origins.len().min(local_screens);
    let me = usize::try_from(start.display_index).ok().filter(|&i| i < shown)?;
    let mut order: Vec<usize> = (0..shown).collect();
    order.sort_by_key(|&i| (start.display_origins[i].x, start.display_origins[i].y));
    order.iter().position(|&i| i == me)
}

/// HDR 会话、本机屏开着 HDR（P1 第 4 项）：合成器收得了 10 位分块画面，就把画面表面标成 BT.2020 + PQ（`white` 是被控端
/// 普通内容的白，尼特），画面原样交给合成器。哪一步不行返回 false，照旧压成普通画面显示
fn hdr_output(win: &mut Window, white: u32) -> bool {
    let tiled = win
        .formats()
        .is_some_and(|f| TILED.iter().any(|m| f.usable.contains(&(XRGB2101010, *m))));
    if !tiled {
        log::warn!("ddpro: pro view shows HDR as normal video: the compositor takes no tiled 10-bit (XRGB2101010) images");
        return false;
    }
    match win.set_hdr(white) {
        Ok(()) => {
            log::info!("ddpro: pro view shows HDR as is (BT.2020 PQ, remote normal white {white} nits)");
            true
        }
        Err(e) => {
            log::warn!("ddpro: pro view shows HDR as normal video: {e}");
            false
        }
    }
}

/// HDR 会话（P1 第 4 项）：着色器按 BT.2020 + PQ 还原（`hdr_white` 是被控端普通内容的白，尼特）。设不上就不用着色器——
/// 视频处理单元那条路做不了 HDR，之后会大声失败而不是显示错的颜色。HDR 原样输出的着色器（10 位输出画面）不用再设
fn hdr_shader(shader: Option<GlConverter>, hdr_white: Option<u32>) -> Option<GlConverter> {
    let mut s = shader?;
    if hdr_white.is_none() || s.format() == XRGB2101010 {
        return Some(s);
    }
    match s.set_hdr(hdr_white) {
        Ok(()) => Some(s),
        Err(e) => {
            log::warn!("ddpro: pro view cannot set up HDR conversion in the shader: {e}");
            None
        }
    }
}

/// 显卡驱动的单帧解码错误常常在等解码完这一步才报出来（iHD：等表面时报解码错误）：丢这一帧、要关键帧，
/// 不当成专业窗口的致命错误。返回这一帧是否解好了
fn wait_decoded(dec: &Decoder, frame: &ddpro::dec::Frame, resync: &AtomicBool) -> bool {
    match dec.wait(frame) {
        Ok(()) => true,
        Err(e) => {
            throttled_log!(
                PEER_LOG_INTERVAL,
                warn,
                "ddpro: pro view decode error while waiting for the GPU, asking for a keyframe: {e}"
            );
            resync.store(true, Ordering::Release);
            false
        }
    }
}

/// 按窗口当前尺寸建输出画面和转换器，把输出画面登记给窗口（换掉原来登记的）。按顺序试（P1 第 7 项）：
/// 1. 着色器用 GBM 按合成器能用的分块排布分配输出画面，视频处理单元导入同一组（要缩放时用它）——着色器画得快
///    （1.75 对 4.1 毫秒），合成器能直接送显示（瘦客户机实测 Y 分块约 100%，视频处理单元自己建的线性画面 0%）；
/// 2. 视频处理单元自己建输出画面（线性），着色器画在上面；
/// 3. 只用视频处理单元。
///
/// `shader` 为假（之前着色器出过错）时直接走第 3 种。
///
/// `hdr`：画面表面已标成 HDR（[`hdr_output`]），先试 10 位输出画面；建不起来就撤掉标记，照旧按上面的顺序。
fn attach(win: &mut Window, dev: &Device, shader: bool, hdr: bool) -> ResultType<(Converter, Option<GlConverter>)> {
    let (w, h) = win.size();
    if shader && hdr {
        let tiled: Vec<u64> = win
            .formats()
            .map(|f| TILED.iter().copied().filter(|m| f.usable.contains(&(XRGB2101010, *m))).collect())
            .unwrap_or_default();
        match hdr_outputs(win, dev, w, h, &tiled) {
            Ok(v) => return Ok(v),
            Err(e) => {
                log::warn!("ddpro: pro view cannot show HDR as is ({e}), showing it as normal video");
                win.unset_hdr();
            }
        }
    }
    if shader {
        let tiled: Vec<u64> = win
            .formats()
            .map(|f| TILED.iter().copied().filter(|m| f.usable.contains(&(XRGB8888, *m))).collect())
            .unwrap_or_default();
        if tiled.is_empty() {
            log::info!("ddpro: pro view: the compositor takes no tiled XRGB8888 images");
        } else {
            match tiled_outputs(win, dev, w, h, &tiled) {
                Ok(v) => return Ok(v),
                Err(e) => log::info!("ddpro: pro view tiled output images unavailable ({e}), using the video processor's"),
            }
        }
    }
    let conv = Converter::new(dev, w, h, SLOTS).map_err(|e| anyhow!("{e}"))?;
    win.reset_slots(SLOTS);
    for slot in 0..SLOTS {
        let image = conv.export(slot).map_err(|e| anyhow!("{e}"))?;
        win.import(slot, &image).map_err(|e| anyhow!("{e}"))?;
    }
    let gl = if shader {
        match GlConverter::on(&conv) {
            Ok(s) => {
                log::info!("ddpro: pro view converts to RGB with shaders on {} (linear output images)", s.renderer());
                Some(s)
            }
            Err(e) => {
                log::warn!("ddpro: pro view shader conversion unavailable, using the video processor: {e}");
                None
            }
        }
    } else {
        None
    };
    Ok((conv, gl))
}

/// HDR 原样输出：GBM 分块的 10 位输出画面，只有着色器画（视频处理单元那条路做不了 HDR，HDR 要缩放时大声失败）。
/// 视频处理单元的转换器照旧建一个、不登记给窗口，只为凑齐调用方的两样东西
fn hdr_outputs(win: &mut Window, dev: &Device, w: u32, h: u32, modifiers: &[u64]) -> Result<(Converter, Option<GlConverter>), String> {
    if modifiers.is_empty() {
        return Err("the compositor takes no tiled 10-bit images".into());
    }
    let gl = GlConverter::new_hdr(w, h, SLOTS, modifiers).map_err(|e| e.to_string())?;
    let images = (0..SLOTS)
        .map(|i| gl.export(i))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let conv = Converter::new(dev, w, h, 1).map_err(|e| e.to_string())?;
    win.reset_slots(SLOTS);
    for (slot, image) in images.iter().enumerate() {
        win.import(slot, image).map_err(|e| e.to_string())?;
    }
    log::info!(
        "ddpro: pro view converts to 10-bit BT.2020 PQ with shaders on {} (output images modifier {:#x})",
        gl.renderer(),
        images[0].modifiers.first().copied().unwrap_or_default()
    );
    Ok((conv, Some(gl)))
}

/// 上面第 1 种：GBM 分块输出画面，着色器和视频处理单元都画进它，窗口登记它
fn tiled_outputs(win: &mut Window, dev: &Device, w: u32, h: u32, modifiers: &[u64]) -> Result<(Converter, Option<GlConverter>), String> {
    let gl = GlConverter::new(w, h, SLOTS, modifiers).map_err(|e| e.to_string())?;
    let images = (0..SLOTS)
        .map(|i| gl.export(i))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let conv = Converter::with_images(dev, w, h, &images).map_err(|e| e.to_string())?;
    win.reset_slots(SLOTS);
    for (slot, image) in images.iter().enumerate() {
        win.import(slot, image).map_err(|e| e.to_string())?;
    }
    log::info!(
        "ddpro: pro view converts to RGB with shaders on {} (output images modifier {:#x})",
        gl.renderer(),
        images[0].modifiers.first().copied().unwrap_or_default()
    );
    Ok((conv, Some(gl)))
}

/// 收包线程：收画面交给主线程（并往 `wake` 写一个字节叫醒它）；主线程要退出时，用户关窗的话先告诉被控端不要了。
fn receive(
    peer: std::net::SocketAddr,
    keys: ddpro::media::Keys,
    frames: Tx<Incoming>,
    wake: UnixStream,
    mut tiles: Decoders,
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
        match r.recv_timeout(RECV_POLL) {
            Ok(None) => {}
            Ok(Some(MediaEvent::Frame {
                data,
                timestamp,
                times,
                changed,
            })) => {
                // 收包时刻在收包线程自己的时间轴上，换成本机单调时钟交给主线程
                let shift = now_ns() - r.now_us() * 1000;
                let times = times.and_then(|t| {
                    Some(Times {
                        captured: t.captured_at()? * 1000 + shift,
                        stages: t.transport_stages()?,
                        last_arrival: t.last_arrival * 1000 + shift,
                    })
                });
                if frames
                    .send(Incoming {
                        data,
                        times,
                        ts: timestamp,
                        changed,
                    })
                    .is_err()
                {
                    break "the window is gone".to_owned();
                }
                // 缓冲满（写不进）说明主线程已经有一个没读的叫醒，不用再写
                (&wake).write(&[1]).ok();
            }
            Ok(Some(MediaEvent::Loss { packets })) => throttled_log!(
                PEER_LOG_INTERVAL,
                info,
                "ddpro: pro view lost {packets} packet(s), keyframe requested"
            ),
            Ok(Some(MediaEvent::Tile { timestamp, index, data })) => tiles.submit(timestamp, index, data),
            Err(e) => break e.to_string(),
        }
        counters.bytes.store(r.stats.bytes, Ordering::Relaxed);
        counters.lost.store(r.stats.lost_packets, Ordering::Relaxed);
        counters.recovered.store(r.stats.recovered, Ordering::Relaxed);
        if let Some(c) = r.clock() {
            counters.rtt_us.store(c.rtt_us.max(0) as u64, Ordering::Relaxed);
        }
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
            Some(message::Union::CursorPosition(p)) => FromSession::CursorPos(p.x, p.y),
            Some(message::Union::Misc(misc)) => match misc.union {
                Some(misc::Union::SwitchDisplay(s)) => FromSession::Display {
                    rect: (s.x, s.y, s.width, s.height),
                    resolutions: s
                        .resolutions
                        .resolutions
                        .iter()
                        .filter(|r| r.width > 0 && r.height > 0)
                        .map(|r| (r.width as u32, r.height as u32))
                        .collect(),
                    original: (
                        s.original_resolution.width.max(0) as u32,
                        s.original_resolution.height.max(0) as u32,
                    ),
                },
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

/// 画远程光标用的叠加图层（悬浮球、菜单、状态块占 0–2）
const LAYER_REMOTE_CURSOR: usize = 3;

/// 被控端的光标被别的东西挪动（USB 透传的数位板、别的连接、程序自己挪）时，在窗口里画出远程光标、藏起本机指针；
/// 本机鼠标一动就换回本机指针。和 RustDesk Flutter 窗口的做法一致：被控端只报「不是本连接挪的」光标位置
/// （`input_service` 排除了本连接自己注入的移动），所以收到位置就说明是别的东西在动。Wayland 下窗口挪不了系统指针，只能画
#[derive(Default)]
struct RemoteCursor {
    image: Option<CursorImage>,
    /// 正画着远程光标（本机指针藏着）
    shown: bool,
    pos: (f64, f64),
}

impl RemoteCursor {
    /// 被控端换了光标形状
    fn set_image(&mut self, win: &mut Window, c: CursorImage) -> ddpro::view::Result<()> {
        self.image = Some(c.clone());
        if self.shown {
            self.draw(win)
        } else {
            win.set_cursor(Some(c))
        }
    }

    /// 被控端报来光标位置（远程桌面坐标）
    fn moved(&mut self, win: &mut Window, input: &Input, x: i32, y: i32) -> ddpro::view::Result<()> {
        match input.window_pos(x, y) {
            Some(p) => {
                self.pos = p;
                if !self.shown {
                    self.shown = true;
                    // 笔和本机鼠标同时动时会来回切，节流
                    throttled_log!(
                        PEER_LOG_INTERVAL,
                        info,
                        "ddpro: pro view: the remote cursor is moved by something else; drawing it at {x},{y}"
                    );
                    win.set_cursor(None)?;
                }
                self.draw(win)
            }
            // 挪到别的屏上了（那块屏的专业窗口去画）
            None => self.local_moved(win),
        }
    }

    /// 本机鼠标动了：换回本机指针
    fn local_moved(&mut self, win: &mut Window) -> ddpro::view::Result<()> {
        if !self.shown {
            return Ok(());
        }
        self.shown = false;
        throttled_log!(PEER_LOG_INTERVAL, info, "ddpro: pro view: back to the local pointer");
        // 先把本机指针放出来：撤图层失败时指针也不至于一直藏着
        let shown = win.set_cursor(self.image.clone());
        win.set_layer(LAYER_REMOTE_CURSOR, None)?;
        shown
    }

    fn draw(&self, win: &mut Window) -> ddpro::view::Result<()> {
        let Some(c) = &self.image else {
            return Ok(());
        };
        let mut bgra = vec![0u8; c.rgba.len()];
        premultiplied_bgra(&c.rgba, &mut bgra);
        win.set_layer(
            LAYER_REMOTE_CURSOR,
            Some(&LayerImage {
                x: self.pos.0 as i32 - c.hotx as i32,
                y: self.pos.1 as i32 - c.hoty as i32,
                width: c.width,
                height: c.height,
                bgra,
            }),
        )
    }
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
    /// 发给远程、还没抬起的鼠标键（RustDesk 的按键位）。按着键时 Wayland 不换指针焦点，按着键还收到「指针离开」
    /// 说明抓取被打断了（弹出系统对话框、进概览），那次松开不会来：离开时替用户抬起，不然远程的键一直按着
    buttons: HashSet<i32>,
    pos: (i32, i32),
    /// 多屏（P1 第 8 项）：指针出了窗口时坐标不夹在这块屏以内。按着键拖出窗口时合成器照旧把指针事件发给这个窗口
    /// （坐标超出窗口，2026-10-21 瘦客户机实测），照同样的比例延伸到远程相邻的屏，远程那边拖着的窗口才能跟到别的屏上
    /// （两边按位置对应，见 `screen_rank`）。窗口里（含缩放留的黑边）照旧夹在这块屏以内
    spill: bool,
}

impl Input {
    fn new(start: &ViewStart, window: (u32, u32), spill: bool) -> Self {
        let mut input = Self {
            window,
            remote: (0, 0, 1, 1),
            fit: (0.0, 0.0, 1.0, 1.0),
            depressed: 0,
            locked: 0,
            held: HashSet::new(),
            buttons: HashSet::new(),
            pos: (start.remote_x, start.remote_y),
            spill,
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

    /// 远程桌面坐标 → 窗口坐标；不在这块屏上返回 None
    fn window_pos(&self, x: i32, y: i32) -> Option<(f64, f64)> {
        let (rx, ry, rw, rh) = self.remote;
        if x < rx || y < ry || x >= rx + rw || y >= ry + rh {
            return None;
        }
        let (fx, fy, fw, fh) = self.fit;
        Some((fx + (x - rx) as f64 * fw / rw as f64, fy + (y - ry) as f64 * fh / rh as f64))
    }

    /// 窗口坐标 → 远程桌面坐标（去掉黑边、按缩放比例，夹在这块屏以内；多屏时指针出了窗口不夹）
    fn remote_pos(&self, x: f64, y: f64) -> (i32, i32) {
        let (fx, fy, fw, fh) = self.fit;
        let (rx, ry, rw, rh) = self.remote;
        let px = ((x - fx) * rw as f64 / fw).floor() as i32;
        let py = ((y - fy) * rh as f64 / fh).floor() as i32;
        let outside = x < 0.0 || y < 0.0 || x >= self.window.0 as f64 || y >= self.window.1 as f64;
        if self.spill && outside {
            return (rx + px, ry + py);
        }
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
                let t = if down {
                    self.buttons.insert(b);
                    MOUSE_TYPE_DOWN
                } else {
                    self.buttons.remove(&b);
                    MOUSE_TYPE_UP
                };
                vec![self.mouse(t | (b << 3), self.pos)]
            }
            Event::PointerLeave => {
                let buttons: Vec<i32> = self.buttons.drain().collect();
                buttons.into_iter().map(|b| self.mouse(MOUSE_TYPE_UP | (b << 3), self.pos)).collect()
            }
            // Wayland 往下 / 往右为正，Flutter 窗口发的是反号（往下滚 -1）
            Event::Wheel { dx, dy } => vec![self.mouse(MOUSE_TYPE_WHEEL, (-dx, -dy))],
            _ => vec![],
        }
    }
}
