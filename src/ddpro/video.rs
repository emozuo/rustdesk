//! 被控端专业视频服务：每块屏一个，和 RustDesk 的普通画面服务同一个模式（按屏按需注册、订阅驱动启停）。
//! 服务线程里：RustDesk 采集器取画面 → NVENC 4:4:4 → 专业画面通道（UDP + SRTP）。
//!
//! 同一块屏专业模式不共享：一块屏同一时间只有一个专业会话（[`SESSIONS`] 按屏登记）。
//! 连接关闭时 RustDesk 在服务器锁内统一退订，服务随之停止；专业通道本身失败（心跳超时）时，
//! 确认连接还订阅着专业服务（即还开着）才把它切回普通画面。

use crate::server::{
    service::{GenericService, Service, ServiceTmpl},
    video_service::{self, VideoSource},
    ConnInner, ServerPtrWeak,
};
use ddpro::{
    enc::{Config, Encoder},
    media::{Keys, Listener, Stamp},
    MediaChannel,
};
use hbb_common::{anyhow::anyhow, bail, log, ResultType};
use scrap::{Capturer, Display, Frame, TraitCapturer};
use std::{
    collections::HashMap,
    io::ErrorKind,
    net::IpAddr,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
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

struct Session {
    conn_id: i32,
    conn: ConnInner,
    server: ServerPtrWeak,
    listener: Mutex<Option<Listener>>,
    keyframe: AtomicBool,
}

lazy_static::lazy_static! {
    /// 按屏登记的专业会话
    static ref SESSIONS: Mutex<HashMap<usize, Arc<Session>>> = Default::default();
}

fn service_name(display: usize) -> String {
    format!("ddpro_monitor{display}")
}

fn general_name(display: usize) -> String {
    video_service::get_service_name(VideoSource::Monitor, display)
}

/// 用这块屏的采集器所在的显卡探测编码能力：编码器之后就建在同一个设备上。
pub fn probe(display: usize) -> Result<ddpro::enc::Caps, ddpro::enc::Error> {
    let cap = capturer(display).map_err(|e| ddpro::enc::Error::Unsupported(e.to_string()))?;
    ddpro::enc::probe(cap.device().device)
}

fn capturer(display: usize) -> ResultType<Capturer> {
    let mut displays = Display::all()?;
    if display >= displays.len() {
        bail!("no display {display} (have {})", displays.len());
    }
    let mut c = Capturer::new(displays.remove(display))?;
    c.set_output_texture(true);
    Ok(c)
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
        listener: Mutex::new(Some(listener)),
        keyframe: AtomicBool::new(false),
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

pub fn request_keyframe(conn_id: i32) {
    for s in SESSIONS.lock().unwrap().values() {
        if s.conn_id == conn_id {
            s.keyframe.store(true, Ordering::Release);
        }
    }
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
    let Some(listener) = session.listener.lock().unwrap().take() else {
        // 通道只能用一次；同一会话重新进来说明上一轮已经结束，等连接退订
        return Ok(());
    };
    let result = stream(&ps, &session, listener);
    match &result {
        // 连接退订（关闭或切走）而结束，RustDesk 已经处理好订阅
        Ok(()) => log::info!("ddpro: pro video on display {display} ended"),
        Err(e) => {
            if back_to_general(display, &session) {
                log::warn!("ddpro: pro video on display {display} failed: {e}; back to general view");
            } else {
                log::warn!("ddpro: pro video on display {display} failed: {e}; the connection is already closed");
            }
        }
    }
    let mut sessions = SESSIONS.lock().unwrap();
    if sessions.get(&display).is_some_and(|s| Arc::ptr_eq(s, &session)) {
        sessions.remove(&display);
    }
    // 失败已经切回普通画面并写了日志；不让服务框架按出错重试
    Ok(())
}

/// 专业通道失败：连接还开着（还订阅着专业服务）才切回普通画面，返回是否切了。连接关闭时 RustDesk
/// 在同一把锁里退订所有服务，所以这里在锁内检查不会和关闭交错。
fn back_to_general(display: usize, session: &Session) -> bool {
    let Some(server) = session.server.upgrade() else {
        return false;
    };
    let mut s = server.write().unwrap();
    let pro = service_name(display);
    if !s.is_subscribed(&pro, session.conn_id) {
        return false;
    }
    s.subscribe(&pro, session.conn.clone(), false);
    s.try_add_monitor_service(display);
    s.subscribe(&general_name(display), session.conn.clone(), true);
    true
}

fn stream(ps: &ProService, session: &Session, listener: Listener) -> ResultType<()> {
    let display = ps.display;
    let (mut cap, first_frame) = start_capture(display)?;
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
    let mut sender = listener.accept(CHANNEL_TIMEOUT).map_err(|e| anyhow!("{e}"))?;
    log::info!(
        "ddpro: pro video on display {display} started: {w}x{h} 4:4:4 8bit {FPS} fps {BITRATE_KBPS} kbps -> {}",
        sender.peer()
    );

    let interval = Duration::from_secs_f64(1.0 / FPS as f64);
    let first = Instant::now();
    let mut i: u64 = 0;
    // 分段延迟只记新采集到的画面：第一帧在通道建立前就取了（含等主控端打招呼的时间），
    // 没有新画面时编码器重复上一帧，这两种都不带时间
    let mut captured = None;
    while ps.ok() {
        if i > 0 {
            let tick = first + interval * i as u32;
            let wait = tick.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
            src = next_texture(&mut cap, wait)?;
            captured = src.map(|_| Instant::now());
        }
        let force = session.keyframe.swap(false, Ordering::AcqRel);
        let f = enc.encode(src, force).map_err(|e| anyhow!("{e}"))?;
        let stamp = captured.map(|captured| Stamp {
            captured,
            encoded: Instant::now(),
        });
        let ts = (i * 90_000 / FPS as u64) as u32;
        sender.send_frame(&f.data, ts, stamp).map_err(|e| anyhow!("{e}"))?;
        if let Some(d) = dump.as_mut() {
            if std::io::Write::write_all(d, &f.data).is_err() {
                dump = None;
            }
        }
        if !sender.peer_alive() {
            bail!("the controller stopped answering on the pro channel");
        }
        i += 1;
        if let Some(d) = (first + interval * i as u32).checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    let st = &sender.stats;
    log::info!(
        "ddpro: pro video on display {display} sent {} frames, {} packets, {} bytes",
        st.frames,
        st.packets,
        st.bytes
    );
    Ok(())
}

/// 建好采集器并取到第一帧（编码器要按第一帧的尺寸创建）。
///
/// 同一个进程里同一块屏同时只能有一路 DXGI 屏幕复制。刚切过来时，这个连接的普通画面服务要到它下一轮
/// 循环才放开这块屏，这期间新建的采集器取不到画面（scrap 报 AddrNotAvailable，2026-10-13 在 209.32 上
/// 实撞）。所以「建采集器 + 取第一帧」整体重试：取不到就丢掉采集器重建，和 RustDesk 普通画面服务遇到
/// 采集错误时重建采集器的做法一致。
fn start_capture(display: usize) -> ResultType<(Capturer, *mut std::ffi::c_void)> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        let attempt = capturer(display).and_then(|mut c| {
            let t0 = Instant::now();
            loop {
                if let Some(t) = next_texture(&mut c, Duration::from_millis(100))? {
                    return Ok((c, t));
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
fn next_texture(cap: &mut Capturer, wait: Duration) -> ResultType<Option<*mut std::ffi::c_void>> {
    match cap.frame(wait) {
        Ok(Frame::Texture((t, _))) if !t.is_null() => Ok(Some(t)),
        Ok(_) => Ok(None),
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(None),
        Err(e) => bail!("capture failed: {e}"),
    }
}

fn texture_size(src: Option<*mut std::ffi::c_void>) -> ResultType<(u32, u32)> {
    let p = src.ok_or_else(|| anyhow!("no frame"))?;
    ddpro::enc::texture_size(p).map_err(|e| anyhow!("{e}"))
}
