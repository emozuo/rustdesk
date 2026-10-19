//! 被控端专业视频服务：每块屏一个，和 RustDesk 的普通画面服务同一个模式（按屏按需注册、订阅驱动启停）。
//! 服务线程里：RustDesk 采集器取画面 → NVENC 4:4:4 → 专业画面通道（UDP + SRTP）。
//!
//! 同一块屏专业模式不共享：一块屏同一时间只有一个专业会话（[`SESSIONS`] 按屏登记）。
//! 连接关闭时 RustDesk 在服务器锁内统一退订，服务随之停止；专业通道本身失败（心跳超时）时，
//! 确认连接还订阅着专业服务（即还开着）才把它切回普通画面。
//!
//! 画面源变了（分辨率、位置、Windows 桌面切换）和 RustDesk 普通画面服务一样处理：用同样的函数发现、
//! 把同样的显示变化消息发给主控端，以出错结束本轮，由服务框架切桌面后重跑。专业通道留在会话里跨重跑保留，
//! 重跑只按新尺寸重建采集器和编码器（P0 实施记录第 5 步「专业模式跟随画面源变化」）。

use crate::{
    privacy_mode::{
        get_privacy_mode_conn_id, is_current_privacy_mode_impl, INVALID_PRIVACY_MODE_CONN_ID,
        PRIVACY_MODE_IMPL_WIN_MAG,
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
    media::{Keys, Listener, Sender, Stamp},
    MediaChannel,
};
use hbb_common::{anyhow::anyhow, bail, log, ResultType};
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
/// 主控端收到「同意」后来打招呼的时限
const CHANNEL_TIMEOUT: Duration = Duration::from_secs(3);
/// 一个采集器建好后第一帧最多等多久（DXGI 开始采集后第一次取帧总会给出整屏）
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(1);
/// 启动阶段（等普通画面服务放开这块屏）总共重试多久
const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
/// 诊断：被控端把发出的码流存到这个文件（和主控端同名的开关，见 `client.rs`）
const DIAG_DUMP: &str = "DDPRO_DIAG_DUMP";
/// 显示变化检查的间隔（和 RustDesk 普通画面服务一样）
const DISPLAY_CHECK_INTERVAL: Duration = Duration::from_secs(1);

struct Session {
    conn_id: i32,
    conn: ConnInner,
    server: ServerPtrWeak,
    /// 专业画面通道；本轮开始时取走，只有画面源变了才放回去给重跑用
    channel: Mutex<Option<Channel>>,
}

enum Channel {
    /// 端口开好了，等主控端来打招呼；过了截止时间主控端就不再连了（它也只等这么久）
    Waiting(Listener, Instant),
    Open(Open),
}

impl Channel {
    /// 还能留给重跑用：已经接通的，或者还没过截止时间的
    fn usable(&self) -> bool {
        match self {
            Channel::Waiting(_, deadline) => Instant::now() < *deadline,
            Channel::Open(_) => true,
        }
    }
}

struct Open {
    sender: Sender,
    /// RTP 时间戳的起点：重跑后时间戳接着走，不回到 0
    epoch: Instant,
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

fn service_name(display: usize) -> String {
    format!("{}{display}", super::server::PRO_SERVICE_PREFIX)
}

fn general_name(display: usize) -> String {
    video_service::get_service_name(VideoSource::Monitor, display)
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

/// 这块屏是否被别的专业会话占着（还订阅着专业服务才算；断掉的会话留下的登记不算）。
pub fn busy_by_other_pro(display: usize, conn_id: i32, server: &ServerPtrWeak) -> bool {
    let Some(other) = SESSIONS.lock().unwrap().get(&display).cloned() else {
        return false;
    };
    if other.conn_id == conn_id {
        return false;
    }
    server
        .upgrade()
        .is_some_and(|s| s.read().unwrap().is_subscribed(&service_name(display), other.conn_id))
}

/// 谈成后开通道、切订阅，返回给主控端的通道信息。
pub fn start(
    display: usize,
    conn_id: i32,
    conn: ConnInner,
    server: ServerPtrWeak,
    local_ip: IpAddr,
) -> ResultType<MediaChannel> {
    // 直连端口监听的是 IPv6 双栈，IPv4 主控端连进来时这一侧的地址是 ::ffff:a.b.c.d；
    // 拿它开 UDP 端口 Windows 报 10049（地址无效），还原成 IPv4（2026-10-13 在 209.32 上实撞）
    let local_ip = local_ip.to_canonical();
    let keys = Keys::generate();
    let listener = Listener::bind(local_ip, keys.clone())?;
    let port = listener.local_addr()?.port();
    let session = Arc::new(Session {
        conn_id,
        conn: conn.clone(),
        server: server.clone(),
        channel: Mutex::new(Some(Channel::Waiting(listener, Instant::now() + CHANNEL_TIMEOUT))),
    });
    SESSIONS.lock().unwrap().insert(display, session);

    let server = server.upgrade().ok_or_else(|| anyhow!("server is gone"))?;
    let mut s = server.write().unwrap();
    let pro = service_name(display);
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
        sp: GenericService::new(service_name(display), false),
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
    let Some(channel) = session.channel.lock().unwrap().take() else {
        // 本会话已经结束，等连接退订
        return Ok(());
    };
    match stream(&ps, channel) {
        Ok((End::SourceChanged(why), channel)) => {
            *session.channel.lock().unwrap() = Some(channel);
            // 放回之后再看一次：这期间连接退订了的话服务框架不会再重跑，通道就地关掉
            if ps.ok() {
                log::info!("ddpro: pro video on display {display}: {why}; restarting capture and encoder");
                // 和 RustDesk 普通画面服务一样以出错结束本轮：服务框架等一会儿、切到当前桌面后重跑
                bail!("SWITCH: {why}");
            }
            session.channel.lock().unwrap().take();
            log::info!("ddpro: pro video on display {display} ended");
        }
        // 连接退订（关闭或切走）而结束，RustDesk 已经处理好订阅
        Ok((End::Unsubscribed, _)) => log::info!("ddpro: pro video on display {display} ended"),
        Ok((End::ControllerClosed, _)) => match back_to_general(display, &session) {
            Back::General => log::info!("ddpro: the controller closed the pro view on display {display}; back to general view"),
            Back::OtherDisplay => log::info!("ddpro: the controller closed the pro view on display {display}; the connection already watches other displays"),
            Back::Closed => {}
        },
        Err(e) => match back_to_general(display, &session) {
            Back::General => {
                log::warn!("ddpro: pro video on display {display} failed: {e}; back to general view");
                tell_controller(&session, &e);
            }
            Back::OtherDisplay => {
                log::warn!("ddpro: pro video on display {display} failed: {e}; the connection already watches other displays");
                tell_controller(&session, &e);
            }
            Back::Closed => log::warn!("ddpro: pro video on display {display} failed: {e}; the connection is already closed"),
        },
    }
    let mut sessions = SESSIONS.lock().unwrap();
    if sessions.get(&display).is_some_and(|s| Arc::ptr_eq(s, &session)) {
        sessions.remove(&display);
    }
    // 失败已经切回普通画面并写了日志；不让服务框架按出错重试
    Ok(())
}

/// 被控端这一侧出错、已退回普通画面：经 RustDesk 原连接告诉主控端原因，它马上关专业窗口、挂提示
/// （不必等画面通道 3 秒没画面才发现，原因也不丢）
fn tell_controller(session: &Session, e: &hbb_common::anyhow::Error) {
    use ddpro::stopped::Reason;
    let reason = match e.downcast_ref::<ChannelError>() {
        Some(ChannelError::Lost) => Reason::CHANNEL_LOST,
        Some(ChannelError::NeverOpened(_)) => Reason::CHANNEL_FAILED,
        None => Reason::VIDEO_ERROR,
    };
    if let Some(msg) = super::wire::wrap(ddpro::stopped(reason, e.to_string())) {
        session.conn.clone().send(Arc::new(msg));
    }
}

/// 画面通道本身的问题（不是采集、编码出错）：告诉主控端时要分开说，它挂的提示不一样
#[derive(Debug)]
enum ChannelError {
    /// 通了之后主控端 3 秒没心跳（防火墙挡了回程、网络断了、主控端那边没了）
    Lost,
    /// 主控端一直没来打招呼
    NeverOpened(String),
}

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelError::Lost => write!(f, "the controller stopped answering on the pro channel"),
            ChannelError::NeverOpened(e) => write!(f, "the controller never opened the pro channel: {e}"),
        }
    }
}

impl std::error::Error for ChannelError {}

enum Back {
    /// 连接已经关了
    Closed,
    /// 切回了这块屏的普通画面
    General,
    /// 连接已经在看别的屏（主控端切过屏，或在普通窗口里选了看多块屏），不订回这块屏
    OtherDisplay,
}

/// 专业会话结束且连接还开着（还订阅着专业服务）：退订专业服务；连接没在看别的屏才订回这块屏的普通画面
/// （主控端切屏后才结束的专业会话，不能把旧屏的普通画面订回来）。连接关闭时 RustDesk 在同一把锁里退订
/// 所有服务，所以这里在锁内检查不会和关闭交错。
fn back_to_general(display: usize, session: &Session) -> Back {
    let Some(server) = session.server.upgrade() else {
        return Back::Closed;
    };
    // 枚举显示器要几毫秒，放在服务器锁外
    let ndisplay = Display::all().map_or(0, |d| d.len()).max(display + 1);
    let mut s = server.write().unwrap();
    let pro = service_name(display);
    let id = session.conn_id;
    if !s.is_subscribed(&pro, id) {
        return Back::Closed;
    }
    s.subscribe(&pro, session.conn.clone(), false);
    let elsewhere = (0..ndisplay)
        .filter(|&i| i != display)
        .any(|i| s.is_subscribed(&general_name(i), id) || s.is_subscribed(&service_name(i), id));
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
    /// 主控端关了专业窗口，要回普通画面
    ControllerClosed,
    /// 画面源变了：通道留着（还没接通的也留着），重跑时接着用
    SourceChanged(String),
}

fn stream(ps: &ProService, channel: Channel) -> ResultType<(End, Channel)> {
    let display = ps.display;
    // 服务线程不在当前桌面（比如被控端停在锁屏界面时开始专业会话）时采集器建不起来：
    // 和画面传输中途一样，交服务框架切到当前桌面后重跑。主控端已经不等了的就不再重跑，按故障退回普通画面
    if desktop_changed() && channel.usable() {
        return Ok((End::SourceChanged("desktop changed".into()), channel));
    }
    let (mut cap, geometry, first_frame) = match start_capture(display) {
        Ok(v) => v,
        Err(e) if desktop_changed() && channel.usable() => {
            return Ok((End::SourceChanged(format!("desktop changed ({e})")), channel))
        }
        Err(e) => return Err(e),
    };
    let dev = cap.device();
    // 诊断：把发出去的码流也存一份，和主控端收到的逐字节比
    let mut dump = std::env::var(DIAG_DUMP)
        .ok()
        .and_then(|p| std::fs::File::create(p).ok());
    let mut src = Some(first_frame);
    let (w, h) = texture_size(src)?;
    let mut enc = Encoder::new(
        dev.device,
        Config {
            width: w,
            height: h,
            fps: FPS,
            bitrate_kbps: BITRATE_KBPS,
            preset: PRESET,
            dither: true,
        },
    )
    .map_err(|e| anyhow!("{e}"))?;
    let Open { mut sender, epoch } = match channel {
        Channel::Waiting(listener, _) => Open {
            sender: listener
                .accept(CHANNEL_TIMEOUT)
                .map_err(|e| ChannelError::NeverOpened(e.to_string()))?,
            epoch: Instant::now(),
        },
        Channel::Open(open) => open,
    };
    log::info!(
        "ddpro: pro video on display {display} started: {w}x{h} 4:4:4 8bit {FPS} fps {BITRATE_KBPS} kbps -> {}",
        sender.peer()
    );

    let interval = Duration::from_secs_f64(1.0 / FPS as f64);
    let first = Instant::now();
    let ts_base = (first - epoch).as_micros() as u64 * 9 / 100;
    let mut i: u64 = 0;
    // 分段延迟只记新采集到的画面：第一帧在通道建立前就取了（含等主控端打招呼的时间），
    // 没有新画面时编码器重复上一帧，这两种都不带时间
    let mut captured = None;
    let mut last_display_check = first;
    let end = loop {
        if !ps.ok() {
            break End::Unsubscribed;
        }
        if mag_privacy_mode() {
            bail!("privacy mode uses the magnifier, which pro capture cannot see through");
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
                    // 和 RustDesk 普通画面服务一样：采集出错先立刻刷新显示列表，是画面源变了就重跑
                    display_service::check_displays_changed().ok();
                    if display_changed(ps, &geometry) {
                        break End::SourceChanged(format!("display changed ({e})"));
                    }
                    if desktop_changed() {
                        break End::SourceChanged(format!("desktop changed ({e})"));
                    }
                    // 屏幕复制被系统收回（解锁 / 登录过程中、切桌面前后都会发生，2026-10-19 在 209.32 上实撞）：
                    // 和 RustDesk 普通画面服务一样重建采集器
                    if e.kind() == ErrorKind::ConnectionReset {
                        break End::SourceChanged(format!("capture access lost ({e})"));
                    }
                    bail!("capture failed: {e}");
                }
            };
            captured = src.map(|_| Instant::now());
        }
        let force = sender.take_keyframe_request();
        let f = enc.encode(src, force).map_err(|e| anyhow!("{e}"))?;
        let stamp = captured.map(|captured| Stamp {
            captured,
            encoded: Instant::now(),
        });
        let ts = (ts_base + i * 90_000 / FPS as u64) as u32;
        sender.send_frame(&f.data, ts, stamp).map_err(|e| anyhow!("{e}"))?;
        if let Some(d) = dump.as_mut() {
            if std::io::Write::write_all(d, &f.data).is_err() {
                dump = None;
            }
        }
        if sender.peer_said_bye() {
            break End::ControllerClosed;
        }
        if !sender.peer_alive() {
            return Err(ChannelError::Lost.into());
        }
        i += 1;
        if let Some(d) = (first + interval * i as u32).checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    };
    if !matches!(end, End::SourceChanged(_)) {
        let st = &sender.stats;
        log::info!(
            "ddpro: pro video on display {display} sent {} frames, {} packets, {} bytes",
            st.frames,
            st.packets,
            st.bytes
        );
    }
    Ok((end, Channel::Open(Open { sender, epoch })))
}

/// Windows 桌面切换了（锁屏、UAC、登录界面）：和 RustDesk 普通画面服务同一个判断
fn desktop_changed() -> bool {
    crate::platform::windows::desktop_changed() && !crate::portable_service::client::running()
}

/// 隐私模式开着且用的是放大镜实现：它靠专门的截屏方式绕开遮罩，专业采集只会拍到黑屏
/// （默认的「截屏时排除」实现不影响专业采集，见 P0 实施记录）。
/// 支持「截屏时排除」的系统上 RustDesk 根本不提供放大镜实现，直接跳过：查隐私模式要拿它的锁，打开隐私模式时
/// 这把锁要被持有好几秒，每帧去等会让专业画面停发、主控端 3 秒后判定断开（2026-10-19 在 209.32 上实撞）
fn mag_privacy_mode() -> bool {
    if crate::privacy_mode::win_exclude_from_capture::is_supported() {
        return false;
    }
    get_privacy_mode_conn_id().is_some_and(|id| id != INVALID_PRIVACY_MODE_CONN_ID)
        && is_current_privacy_mode_impl(PRIVACY_MODE_IMPL_WIN_MAG)
}

/// 和 RustDesk 普通画面服务同一个检查；变了就把同一条显示变化消息发给主控端
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
/// 同一个进程里同一块屏同时只能有一路 DXGI 屏幕复制。刚切过来时，这个连接的普通画面服务要到它下一轮
/// 循环才放开这块屏，这期间新建的采集器取不到画面（scrap 报 AddrNotAvailable，2026-10-13 在 209.32 上
/// 实撞）。所以「建采集器 + 取第一帧」整体重试：取不到就丢掉采集器重建，和 RustDesk 普通画面服务遇到
/// 采集错误时重建采集器的做法一致。
fn start_capture(display: usize) -> ResultType<(Capturer, Geometry, *mut std::ffi::c_void)> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        let attempt = capturer(display).and_then(|(mut c, g)| {
            let t0 = Instant::now();
            loop {
                if let Some(t) = next_texture(&mut c, Duration::from_millis(100))
                    .map_err(|e| anyhow!("capture failed: {e}"))?
                {
                    return Ok((c, g, t));
                }
                if t0.elapsed() > FIRST_FRAME_TIMEOUT {
                    bail!("no first frame from display {display}");
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

fn texture_size(src: Option<*mut std::ffi::c_void>) -> ResultType<(u32, u32)> {
    let p = src.ok_or_else(|| anyhow!("no frame"))?;
    ddpro::enc::texture_size(p).map_err(|e| anyhow!("{e}"))
}
