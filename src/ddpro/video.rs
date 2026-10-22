//! 被控端专业视频服务：每块屏一个，和 RustDesk 的通用画面服务同一个模式（按屏按需注册、订阅驱动启停）。
//! 服务线程里：RustDesk 采集器取画面 → NVENC 4:4:4 → 专业画面通道（UDP + SRTP）。
//!
//! 同一块屏专业模式不共享：一块屏同一时间只有一个专业会话（[`SESSIONS`] 按屏登记）。
//! 连接关闭时 RustDesk 在服务器锁内统一退订，服务随之停止；专业通道本身失败（心跳超时）时，
//! 确认连接还订阅着专业服务（即还开着）才把它切回通用画面。
//!
//! 画面源变了（分辨率、位置、Windows 桌面切换）和 RustDesk 通用画面服务一样处理：用同样的函数发现、
//! 把同样的显示变化消息发给主控端，等一会儿、切到当前桌面后在本线程里重建（退避和 RustDesk 服务框架的出错重跑
//! 一样）。专业通道跨重建保留，只按新尺寸重建采集器和编码器（P0 实施记录第 5 步「专业模式跟随画面源变化」）。

use crate::{
    ddpro::{lossless::Lossless, server::pro_service_name},
    privacy_mode::{
        get_privacy_mode_conn_id, is_current_privacy_mode_impl, DEFAULT_PRIVACY_MODE_IMPL,
        INVALID_PRIVACY_MODE_CONN_ID, PRIVACY_MODE_IMPL_WIN_EXCLUDE_FROM_CAPTURE, PRIVACY_MODE_IMPL_WIN_MAG,
    },
    server::{
        display_service,
        service::{GenericService, Service, ServiceTmpl, Subscriber},
        video_service::{self, VideoSource},
        ConnInner, ServerPtrWeak,
    },
};
use ddpro::{
    enc::{Config, Encoder},
    media::{tile::TILE, Keys, Listener, Sender, Stamp},
    stopped::Reason,
    MediaChannel,
};
use hbb_common::{anyhow::anyhow, bail, log, throttled_log, ResultType};
use scrap::{Capturer, Display, Frame, TraitCapturer};
use std::{
    collections::HashMap,
    io::ErrorKind,
    net::IpAddr,
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const FPS: u32 = 60;
const BITRATE_KBPS: u32 = 30_000;
const PRESET: u8 = 4;
/// 静止无损补发的预算上限：内网实测 800 Mbps 突发不丢包，给一半（P1 实施记录第 7 节）；链路扛不住时带宽保护往下调（`lossless.rs`）
const LOSSLESS_KBPS: u32 = 400_000;
/// 静止不编码（P1 第 9 项）：大变化之后画面内容不变了，照旧重编这么多帧再停——给用不上无损补块的主控端（缩放显示）留画质收敛。
/// 和 RustDesk 通用模式的「静止刷新」同一思路（`video_service/static_refresh.rs`：停下后按实测预算重复编码再停），
/// 它给 H.265 NVENC 最高画质档的预算是 30 帧；P1 第 1 项实测静止后第 30 帧 PSNR 58 dB
const STILL_TAIL: u32 = 30;
/// 小变化（不到八分之一屏：任务栏时钟、光标闪烁、小图标）只重编这么多帧：小块区域第一帧就编得清楚，1:1 显示时还马上被无损补齐。
/// 按 STILL_TAIL 收尾的话时钟每秒跳一次编码器就停不下来（2026-10-21 夜 209.32 实测：两块屏任务栏每秒变 2 块，「静止」时编码器仍占 20–26%）
const STILL_TAIL_SMALL: u32 = 8;
const _: () = assert!(STILL_TAIL >= STILL_TAIL_SMALL);
/// 停编期间每隔这么久还编一帧（重复画面）：主控端 3 秒收不到画面包就判通道断了；这一帧也让主控端发现停编前最后一帧丢的包、要求补发
const STILL_KEEPALIVE: Duration = Duration::from_secs(1);
/// 主控端收到「同意」后来打招呼的时限
const CHANNEL_TIMEOUT: Duration = Duration::from_secs(3);
/// 一个采集器建好后第一帧最多等多久（DXGI 开始采集后第一次取帧总会给出整屏）
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(1);
/// 启动阶段（等通用画面服务放开这块屏）总共重试多久
const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
/// 诊断：不做静止无损（只发视频，对比画质、排查问题用）
const DIAG_NO_LOSSLESS: &str = "DDPRO_DIAG_NO_LOSSLESS";
/// 诊断：停编期间心跳帧的间隔（毫秒，默认 [`STILL_KEEPALIVE`]），量「隔多久编一帧显卡才不降频」用；0 = 不停编（和改之前一样每帧编）
const DIAG_STILL_KEEPALIVE_MS: &str = "DDPRO_DIAG_STILL_KEEPALIVE_MS";
/// 显示变化检查的间隔（和 RustDesk 通用画面服务一样）
const DISPLAY_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// 画面源变了之后重建前等多久：和 RustDesk 服务框架出错重跑的退避一样（30 毫秒起、加倍、最多 1 秒）
const REBUILD_BACKOFF_MIN: Duration = Duration::from_millis(30);
const REBUILD_BACKOFF_MAX: Duration = Duration::from_secs(1);
/// 采集被系统反复收回时会一直重建（没有次数上限，见 P1 已知限制），这行日志节流
const REBUILD_LOG_INTERVAL: Duration = Duration::from_secs(10);

struct Session {
    conn: ConnInner,
    server: ServerPtrWeak,
    /// 谈成时开好的专业画面通道，服务线程开始时取走
    channel: Mutex<Option<Channel>>,
    /// 谈成的位深：8，或 10（P1 第 4 项，只在屏开着 HDR 时谈得到）
    bit_depth: u32,
    /// HDR 会话：采集器交出 scRGB 原样、按 BT.2020 + PQ 编（谈成后桌面关了 HDR 就按 10bit 普通画面编）
    hdr: bool,
}

enum Channel {
    /// 端口开好了，等主控端来打招呼；过了截止时间主控端就不再连了（它也只等这么久）
    Waiting(Listener, Instant),
    Open(Sender),
}

impl Channel {
    /// 还值得重建接着用：已经接通的，或者还没过截止时间的
    fn usable(&self) -> bool {
        match self {
            Channel::Waiting(_, deadline) => Instant::now() < *deadline,
            Channel::Open(_) => true,
        }
    }
}

/// 采集器建立时这块屏的情况，用来和 RustDesk 的显示列表比对有没有变化
struct Geometry {
    ndisplay: usize,
    origin: (i32, i32),
    width: usize,
    height: usize,
}

lazy_static::lazy_static! {
    /// 按屏登记的专业会话
    static ref SESSIONS: Mutex<HashMap<usize, Arc<Session>>> = Default::default();
}

fn general_name(display: usize) -> String {
    video_service::get_service_name(VideoSource::Monitor, display)
}

/// 这块屏此刻开着 HDR 时，普通内容的白有多少尼特（P1 第 4 项）；没开（或没这块屏）返回 None
pub fn hdr_white(display: usize) -> Option<u32> {
    let d = Display::all().ok()?.into_iter().nth(display)?;
    ddpro::enc::hdr_sdr_white_nits(&d.name())
}

/// 用这块屏的采集器所在的显卡探测编码能力：编码器之后就建在同一个设备上。
pub fn probe(display: usize) -> Result<ddpro::enc::Caps, ddpro::enc::Error> {
    let (cap, _) = capturer(display).map_err(|e| ddpro::enc::Error::Unsupported(e.to_string()))?;
    ddpro::enc::probe(cap.device().device)
}

fn capturer(display: usize) -> ResultType<(Capturer, Geometry)> {
    let mut displays = Display::all()?;
    let ndisplay = displays.len();
    if display >= ndisplay {
        bail!("no display {display} (have {ndisplay})");
    }
    let d = displays.remove(display);
    let geometry = Geometry {
        ndisplay,
        origin: d.origin(),
        width: d.width(),
        height: d.height(),
    };
    let mut c = Capturer::new(d)?;
    c.set_output_texture(true);
    Ok((c, geometry))
}

/// 这块屏登记的专业会话属于别的连接时，返回那个连接（它是否还订阅着专业服务由调用方在服务器锁里查）。
pub fn other_session(display: usize, conn_id: i32) -> Option<i32> {
    SESSIONS.lock().unwrap().get(&display).map(|o| o.conn.id()).filter(|&id| id != conn_id)
}

/// 开通道时发现这块屏已被别的连接占着：谈判前查过一次没人，但两台主控端几乎同时请求同一块屏时，
/// 先登记的那个在这之间占了屏。答复按「屏幕忙」拒绝，不按「开不了」。
#[derive(Debug)]
pub struct Busy(pub String);

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Busy {}

/// 谈成后开通道、切订阅，返回给主控端的通道信息。
/// 「这块屏有没有被别的连接占着」的检查和登记、切订阅在同一段服务器锁里做，两台主控端并发请求时只有一个成；
/// 锁的先后固定为服务器锁在外、[`SESSIONS`] 在内。
/// `bit_depth` / `hdr` 是谈成的格式。
pub fn start(
    display: usize,
    conn_id: i32,
    conn: ConnInner,
    server: ServerPtrWeak,
    local_ip: IpAddr,
    bit_depth: u32,
    hdr: bool,
) -> ResultType<MediaChannel> {
    // 直连端口监听的是 IPv6 双栈，IPv4 主控端连进来时这一侧的地址是 ::ffff:a.b.c.d；
    // 拿它开 UDP 端口 Windows 报 10049（地址无效），还原成 IPv4（2026-10-13 在 209.32 上实撞）
    let local_ip = local_ip.to_canonical();
    let keys = Keys::generate();
    let listener = Listener::bind(local_ip, keys.clone())?;
    let port = listener.local_addr()?.port();
    let session = Arc::new(Session {
        conn: conn.clone(),
        server: server.clone(),
        channel: Mutex::new(Some(Channel::Waiting(listener, Instant::now() + CHANNEL_TIMEOUT))),
        bit_depth,
        hdr,
    });

    let server = server.upgrade().ok_or_else(|| anyhow!("server is gone"))?;
    let mut s = server.write().unwrap();
    if let Some(why) = super::server::busy(&s, display, conn_id) {
        return Err(Busy(why).into());
    }
    let pro = pro_service_name(display);
    SESSIONS.lock().unwrap().insert(display, session);
    if !s.contains(&pro) {
        s.add_service(Box::new(new_service(display)));
    }
    s.subscribe(&general_name(display), conn.clone(), false);
    s.subscribe(&pro, conn, true);
    Ok(ddpro::media_channel(local_ip, port, &keys))
}

#[derive(Clone)]
struct ProService {
    sp: GenericService,
    display: usize,
}

impl Deref for ProService {
    type Target = ServiceTmpl<ConnInner>;

    fn deref(&self) -> &Self::Target {
        &self.sp
    }
}

impl DerefMut for ProService {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sp
    }
}

fn new_service(display: usize) -> GenericService {
    let ps = ProService {
        sp: GenericService::new(pro_service_name(display), false),
        display,
    };
    GenericService::run(&ps, run);
    ps.sp
}

fn run(ps: ProService) -> ResultType<()> {
    let display = ps.display;
    let Some(session) = SESSIONS.lock().unwrap().get(&display).cloned() else {
        return Ok(());
    };
    let Some(mut channel) = session.channel.lock().unwrap().take() else {
        // 本会话已经结束，等连接退订
        return Ok(());
    };
    // 画面源变了就在这里重建，通道一直留在本线程：连接退订后自然随本轮结束释放。（交给服务框架按出错重跑的话，
    // 等待期间连接退订，框架不再重跑，放回会话里的通道就没人关了。）
    let mut backoff = REBUILD_BACKOFF_MIN;
    let result = loop {
        let round = Instant::now();
        match stream(&ps, channel, session.bit_depth, session.hdr) {
            Ok((End::SourceChanged(why), ch)) if ps.ok() => {
                throttled_log!(
                    REBUILD_LOG_INTERVAL,
                    info,
                    "ddpro: pro video on display {display}: {why}; restarting capture and encoder"
                );
                // 和 RustDesk 服务框架出错重跑一样：上一轮跑得久就从头退避，否则加倍；等一会儿、切到当前桌面再重建
                backoff = if round.elapsed() > REBUILD_BACKOFF_MAX {
                    REBUILD_BACKOFF_MIN
                } else {
                    (backoff * 2).min(REBUILD_BACKOFF_MAX)
                };
                std::thread::sleep(backoff);
                crate::platform::windows::try_change_desktop();
                channel = ch;
            }
            other => break other,
        }
    };
    match result {
        // 连接退订（关闭或切走）而结束，RustDesk 已经处理好订阅
        Ok((End::Unsubscribed, _)) | Ok((End::SourceChanged(_), _)) => {
            log::info!("ddpro: pro video on display {display} ended")
        }
        Ok((End::ControllerClosed, _)) => match back_to_general(display, &session) {
            Back::General => {
                log::info!("ddpro: the controller closed the pro view on display {display}; back to general view");
                tell_released(&session, display);
            }
            Back::OtherDisplay => {
                log::info!("ddpro: the controller closed the pro view on display {display}; the connection already watches other displays");
                tell_released(&session, display);
            }
            Back::Superseded => log::info!("ddpro: the pro view on display {display} ended; a newer pro session took the display"),
            Back::Closed => {}
        },
        Err(e) => match back_to_general(display, &session) {
            Back::General => {
                log::warn!("ddpro: pro video on display {display} failed: {e}; back to general view");
                tell_controller(&session, display, &e);
            }
            Back::OtherDisplay => {
                log::warn!("ddpro: pro video on display {display} failed: {e}; the connection already watches other displays");
                tell_controller(&session, display, &e);
            }
            Back::Superseded => log::warn!("ddpro: pro video on display {display} failed: {e}; a newer pro session took the display"),
            Back::Closed => log::warn!("ddpro: pro video on display {display} failed: {e}; the connection is already closed"),
        },
    }
    let mut sessions = SESSIONS.lock().unwrap();
    if sessions.get(&display).is_some_and(|s| Arc::ptr_eq(s, &session)) {
        sessions.remove(&display);
    }
    // 失败已经切回通用画面并写了日志；不让服务框架按出错重跑
    Ok(())
}

/// 被控端这一侧出错、已退回通用画面：经 RustDesk 原连接告诉主控端原因，它马上关专业窗口、挂提示
/// （不必等画面通道 3 秒没画面才发现，原因也不丢）
/// 带上屏号：主控端切过屏的话，它据此认出这是旧屏的消息、不去动新屏的专业窗口
fn tell_controller(session: &Session, display: usize, e: &hbb_common::anyhow::Error) {
    let reason = e.downcast_ref::<Stop>().map_or(Reason::VIDEO_ERROR, |s| s.0);
    if let Some(msg) = super::wire::wrap(ddpro::stopped(reason, e.to_string(), display as u32)) {
        session.conn.clone().send(Arc::new(msg));
    }
}

/// 主控端关了这块屏的专业窗口、本会话已退订专业服务：告诉主控端这块屏放回来了。多屏时这块屏的窗口还在的话，
/// 主控端会把它的普通画面要回去（[`back_to_general`] 看到连接还在看别的屏就不订回来，那是单窗口切屏的情形）
fn tell_released(session: &Session, display: usize) {
    if let Some(msg) = super::wire::wrap(ddpro::released(display as u32)) {
        session.conn.clone().send(Arc::new(msg));
    }
}

/// 要单独告诉主控端原因的故障（它挂的提示按原因分）；其余采集、编码出错都按 VIDEO_ERROR 报：
/// - CHANNEL_LOST：通了之后主控端 3 秒没心跳（防火墙挡了回程、网络断了、主控端那边没了）
/// - CHANNEL_FAILED：主控端一直没来打招呼
/// - NO_IMAGE：建了采集器一直没有第一帧（常见于没接显示器——通用画面这时会改用 CPU 截屏，专业画面没有这条路）
/// - PRIVACY_MODE：隐私模式开着且用的是放大镜实现（见 [`mag_privacy_mode`]）
#[derive(Debug)]
struct Stop(Reason, String);

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}

impl std::error::Error for Stop {}

enum Back {
    /// 连接已经关了
    Closed,
    /// 切回了这块屏的通用画面
    General,
    /// 连接已经在看别的屏（主控端切过屏，或在通用窗口里选了看多块屏），不订回这块屏
    OtherDisplay,
    /// 这块屏已经登记了新的专业会话（主控端快速切走又切回）：订阅归它，本会话什么都不动、也不对主控端报停——
    /// 主控端此刻又在这块屏上，报了会把新会话的专业窗口关掉
    Superseded,
}

/// 专业会话结束且连接还开着（还订阅着专业服务）：退订专业服务；连接没在看别的屏才订回这块屏的通用画面
/// （主控端切屏后才结束的专业会话，不能把旧屏的通用画面订回来）。连接关闭时 RustDesk 在同一把锁里退订
/// 所有服务，所以这里在锁内检查不会和关闭交错。
fn back_to_general(display: usize, session: &Session) -> Back {
    let Some(server) = session.server.upgrade() else {
        return Back::Closed;
    };
    // 枚举显示器要几毫秒，放在服务器锁外
    let ndisplay = Display::all().map_or(0, |d| d.len()).max(display + 1);
    let mut s = server.write().unwrap();
    // 主控端快速切走又切回（A→B→A）时，这块屏已经登记了新的专业会话，专业服务的订阅归它，不能退。
    // 登记在服务器锁里做（见 [`start`]），所以在锁内查是准的
    if !SESSIONS.lock().unwrap().get(&display).is_some_and(|s| std::ptr::eq(Arc::as_ptr(s), session)) {
        return Back::Superseded;
    }
    let pro = pro_service_name(display);
    let id = session.conn.id();
    if !s.is_subscribed(&pro, id) {
        return Back::Closed;
    }
    s.subscribe(&pro, session.conn.clone(), false);
    let elsewhere = (0..ndisplay)
        .filter(|&i| i != display)
        .any(|i| s.is_subscribed(&general_name(i), id) || s.is_subscribed(&pro_service_name(i), id));
    if elsewhere {
        return Back::OtherDisplay;
    }
    s.try_add_monitor_service(display);
    s.subscribe(&general_name(display), session.conn.clone(), true);
    Back::General
}

/// 专业画面为什么结束本轮
enum End {
    /// 连接退订了专业服务（关闭或切走）
    Unsubscribed,
    /// 主控端关了专业窗口，要回通用画面
    ControllerClosed,
    /// 画面源变了：通道留着（还没接通的也留着），重建后接着用
    SourceChanged(String),
}

fn stream(ps: &ProService, channel: Channel, bit_depth: u32, hdr: bool) -> ResultType<(End, Channel)> {
    let display = ps.display;
    // 服务线程不在当前桌面（比如被控端停在锁屏界面时开始专业会话）时采集器建不起来：
    // 和画面传输中途一样，切到当前桌面后重建。主控端已经不等了的就不再重建，按故障退回通用画面
    if desktop_changed() && channel.usable() {
        return Ok((End::SourceChanged("desktop changed".into()), channel));
    }
    let (mut cap, geometry, first_frame) = match start_capture(display, hdr) {
        Ok(v) => v,
        Err(e) if desktop_changed() && channel.usable() => {
            return Ok((End::SourceChanged(format!("desktop changed ({e})")), channel))
        }
        Err(e) => return Err(e),
    };
    let dev = cap.device();
    let (w, h) = ddpro::enc::texture_size(first_frame).map_err(|e| anyhow!("{e}"))?;
    let mut src = Some(first_frame);
    // 静止无损补发的是源画面的 8bit 原始像素：HDR 的源是 scRGB，不补
    let mut lossless = match std::env::var_os(DIAG_NO_LOSSLESS) {
        None if !hdr => Some(Lossless::new(w, h, LOSSLESS_KBPS, FPS)?),
        _ => None,
    };
    let mut enc = Encoder::new(
        dev.device,
        Config {
            width: w,
            height: h,
            fps: FPS,
            bitrate_kbps: BITRATE_KBPS,
            preset: PRESET,
            // 8bit 桌面转 8bit YUV 只需四舍五入（Sunshine 同样不抖动），实测抖动反而误差更大、码流更大
            dither: false,
            tile: if lossless.is_some() { TILE } else { 0 },
            ten_bit: bit_depth == 10,
            hdr,
        },
    )
    .map_err(|e| anyhow!("{e}"))?;
    let mut sender = match channel {
        // 重建后接着等的只等剩下的时间：主控端过了截止时间就不再来了
        Channel::Waiting(listener, deadline) => listener
            .accept(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| Stop(Reason::CHANNEL_FAILED, format!("the controller never opened the pro channel: {e}")))?,
        Channel::Open(sender) => sender,
    };
    sender.pace(BITRATE_KBPS, FPS);
    if lossless.is_some() {
        sender.pace_tiles(LOSSLESS_KBPS, FPS);
    }
    log::info!(
        "ddpro: pro video on display {display} started: {w}x{h} 4:4:4 {bit_depth}bit{} {FPS} fps {BITRATE_KBPS} kbps, lossless {} -> {}",
        if hdr { " HDR" } else { "" },
        if lossless.is_some() { "on" } else { "off" },
        sender.peer()
    );

    let interval = Duration::from_secs_f64(1.0 / FPS as f64);
    let first = Instant::now();
    // RTP 时间戳从通道的时间零点算：重建后接着走，不回到 0
    let ts_base = (first - sender.epoch()).as_micros() as u64 * 9 / 100;
    let mut i: u64 = 0;
    // 分段延迟只记新采集到的画面：第一帧在通道建立前就取了（含等主控端打招呼的时间），
    // 没有新画面时编码器重复上一帧，这两种都不带时间
    let mut captured = None;
    // 静止不编码：画面内容连续多少个节拍没变、这一段变化要重编多少帧收尾、上次真编码是什么时候
    let mut still: u32 = 0;
    let mut tail = STILL_TAIL;
    let tiles = w.div_ceil(TILE) * h.div_ceil(TILE);
    let keepalive = std::env::var(DIAG_STILL_KEEPALIVE_MS)
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(STILL_KEEPALIVE, Duration::from_millis);
    let mut last_encode = first;
    let mut last_display_check = first;
    let end = loop {
        if !ps.ok() {
            break End::Unsubscribed;
        }
        if mag_privacy_mode() {
            return Err(Stop(
                Reason::PRIVACY_MODE,
                "privacy mode uses the magnifier, which pro capture cannot see through".into(),
            )
            .into());
        }
        if desktop_changed() {
            break End::SourceChanged("desktop changed".into());
        }
        if last_display_check.elapsed() >= DISPLAY_CHECK_INTERVAL {
            last_display_check = Instant::now();
            if display_changed(ps, &geometry) {
                break End::SourceChanged("display changed".into());
            }
        }
        if i > 0 {
            let tick = first + interval * i as u32;
            let wait = tick.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
            src = match next_texture(&mut cap, wait) {
                Ok(t) => t,
                Err(e) => {
                    // 和 RustDesk 通用画面服务一样：采集出错先立刻刷新显示列表，是画面源变了就重建
                    display_service::check_displays_changed().ok();
                    if display_changed(ps, &geometry) {
                        break End::SourceChanged(format!("display changed ({e})"));
                    }
                    if desktop_changed() {
                        break End::SourceChanged(format!("desktop changed ({e})"));
                    }
                    // 屏幕复制被系统收回（解锁 / 登录过程中、切桌面前后都会发生，2026-10-19 在 209.32 上实撞）：
                    // 和 RustDesk 通用画面服务一样重建采集器
                    if e.kind() == ErrorKind::ConnectionReset {
                        break End::SourceChanged(format!("capture access lost ({e})"));
                    }
                    bail!("capture failed: {e}");
                }
            };
            captured = src.map(|_| Instant::now());
        }
        // 先载入、按方块比，再定编不编：采集说「有新画面」不等于内容变了——锁屏界面、某些程序会一直刷新同样的画面
        // （2026-10-21 在 209.32 上实测：锁屏时每秒 60 帧「新画面」、一块都没变）。不按方块比（关了无损）时只能信采集
        let changed = match src {
            Some(t) => enc.load(t).map_err(|e| anyhow!("{e}"))?,
            None => enc.unchanged(),
        };
        let moved = changed.as_ref().map_or(src.is_some(), |b| b.iter().any(|&x| x != 0));
        if moved {
            let n: u32 = changed.as_ref().map_or(tiles, |b| b.iter().map(|x| x.count_ones()).sum());
            let need = if n * 8 >= tiles { STILL_TAIL } else { STILL_TAIL_SMALL };
            // 上一段变化已经收完尾（停编了或正在停编）就从这次重新算；还在收尾就取大的（这一段里有过大变化就按大的收）
            tail = if still > tail { need } else { tail.max(need) };
            still = 0;
        } else {
            still = still.saturating_add(1);
        }
        let force = sender.take_keyframe_request();
        if force {
            // 关键帧（主控端救不回丢包、解码出错）之后整屏按大变化收尾
            still = 0;
            tail = STILL_TAIL;
        }
        let ts = (ts_base + i * 90_000 / FPS as u64) as u32;
        // 静止不编码（P1 第 9 项）：画面停了一阵就不再每帧重编同一张画面，硬件编码器空出来（多屏时给别的屏用、也给用户自己的
        // 程序用）；主控端要关键帧时照编，隔一阵编一帧当心跳。无损补块照常在每个节拍补
        let idle = !force && still > tail && last_encode.elapsed() < keepalive;
        let mut idr = false;
        if !idle {
            let f = enc.encode_loaded(force).map_err(|e| anyhow!("{e}"))?;
            last_encode = Instant::now();
            idr = f.is_idr;
            let stamp = captured.map(|captured| Stamp {
                captured,
                encoded: last_encode,
            });
            if let Some(bitmap) = lossless.as_mut().and_then(|l| l.changed(changed, idr)) {
                sender.send_changed(ts, &bitmap).map_err(|e| anyhow!("{e}"))?;
            }
            sender.send_frame(&f.data, ts, stamp).map_err(|e| anyhow!("{e}"))?;
        }
        if let Some(l) = lossless.as_mut() {
            l.pump(&mut enc, &mut sender, ts, idr)?;
            // 无损方块还没补完就不算静止：主控端只在画新的一帧时把放好的方块一起画出来，停编了的话补上的方块要等下一个心跳帧
            // （最多 1 秒）才上屏。补完之后再按变化大小收尾，跟在最后一批方块后面的帧把它们画出来
            if !l.complete() {
                still = 0;
            }
        }
        if sender.peer_said_bye() {
            break End::ControllerClosed;
        }
        if !sender.peer_alive() {
            return Err(Stop(Reason::CHANNEL_LOST, "the controller stopped answering on the pro channel".into()).into());
        }
        // 落后了（关键帧按发包节奏摊开发要一百多毫秒）就跳过错过的节拍，不背靠背补出一串帧
        let due = (first.elapsed().as_nanos() / interval.as_nanos()) as u64;
        i = (i + 1).max(due);
        if let Some(d) = (first + interval * i as u32).checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    };
    if !matches!(end, End::SourceChanged(_)) {
        let st = &sender.stats;
        log::info!(
            "ddpro: pro video on display {display} sent {} frames, {} packets, {} bytes, {} retransmitted, {} lossless tiles ({} bytes)",
            st.frames,
            st.packets,
            st.bytes,
            st.retransmitted,
            st.tiles,
            st.tile_bytes
        );
    }
    Ok((end, Channel::Open(sender)))
}

/// Windows 桌面切换了（锁屏、UAC、登录界面）：和 RustDesk 通用画面服务同一个判断
fn desktop_changed() -> bool {
    crate::platform::windows::desktop_changed() && !crate::portable_service::client::running()
}

/// 隐私模式开着且用的是放大镜实现：它靠专门的截屏方式绕开遮罩，专业采集只会拍到黑屏
/// （默认的「截屏时排除」实现不影响专业采集，见 P0 实施记录）。
/// 默认实现是「截屏时排除」（系统支持它）时 RustDesk 根本不提供放大镜实现，直接跳过：查隐私模式要拿它的锁，
/// 打开隐私模式时这把锁要被持有好几秒，每帧去等会让专业画面停发、主控端 3 秒后判定断开（2026-10-19 在 209.32 上实撞）。
/// 不支持的老系统（Windows 10 2004 以下）照旧每帧查，打开隐私模式时专业画面仍会断（已知限制）
fn mag_privacy_mode() -> bool {
    if *DEFAULT_PRIVACY_MODE_IMPL == PRIVACY_MODE_IMPL_WIN_EXCLUDE_FROM_CAPTURE {
        return false;
    }
    get_privacy_mode_conn_id().is_some_and(|id| id != INVALID_PRIVACY_MODE_CONN_ID)
        && is_current_privacy_mode_impl(PRIVACY_MODE_IMPL_WIN_MAG)
}

/// 和 RustDesk 通用画面服务同一个检查；变了就把同一条显示变化消息发给主控端
fn display_changed(ps: &ProService, g: &Geometry) -> bool {
    let Some(info) = display_service::check_display_changed(
        g.ndisplay,
        ps.display,
        (g.origin.0, g.origin.1, g.width, g.height),
    ) else {
        return false;
    };
    if let Some(msg) = video_service::make_display_changed_msg(ps.display, Some(info), VideoSource::Monitor) {
        ps.sp.send(msg);
    }
    true
}

/// 建好采集器并取到第一帧（编码器要按第一帧的尺寸创建）。
///
/// 同一个进程里同一块屏同时只能有一路 DXGI 屏幕复制。刚切过来时，这个连接的通用画面服务要到它下一轮
/// 循环才放开这块屏，这期间新建的采集器取不到画面（scrap 报 AddrNotAvailable，2026-10-13 在 209.32 上
/// 实撞）。所以「建采集器 + 取第一帧」整体重试：取不到就丢掉采集器重建，和 RustDesk 通用画面服务遇到
/// 采集错误时重建采集器的做法一致。
/// `hdr`：HDR 会话，采集器把 scRGB 画面原样交出（不压成 8bit）
fn start_capture(display: usize, hdr: bool) -> ResultType<(Capturer, Geometry, *mut std::ffi::c_void)> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        let attempt = capturer(display).and_then(|(mut c, g)| {
            c.set_hdr_passthrough(hdr);
            let t0 = Instant::now();
            loop {
                if let Some(t) = next_texture(&mut c, Duration::from_millis(100))
                    .map_err(|e| anyhow!("capture failed: {e}"))?
                {
                    return Ok((c, g, t));
                }
                if t0.elapsed() > FIRST_FRAME_TIMEOUT {
                    return Err(Stop(Reason::NO_IMAGE, format!("no first frame from display {display}")).into());
                }
            }
        });
        match attempt {
            Ok(r) => return Ok(r),
            Err(e) if Instant::now() < deadline => {
                log::debug!("ddpro: capture on display {display} not ready yet: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e),
        }
    }
}

/// 取一帧显卡纹理；这段时间画面没变返回 None（编码器会重编上一帧）。
fn next_texture(cap: &mut Capturer, wait: Duration) -> std::io::Result<Option<*mut std::ffi::c_void>> {
    match cap.frame(wait) {
        Ok(Frame::Texture((t, _))) if !t.is_null() => Ok(Some(t)),
        Ok(_) => Ok(None),
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
        Err(e) => Err(e),
    }
}
