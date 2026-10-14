//! 专业画面通道：UDP 直连 + 标准 RTP + 标准 SRTP（AES-128-GCM，被篡改的包直接丢弃）。
//!
//! 建立方式：被控端在 RustDesk 已加密的连接里把自己的地址、一次性令牌和密钥告诉主控端；主控端先往被控端
//! 发「打招呼」包（带令牌、同样加密），被控端认准第一个带对令牌的来源地址，之后把画面发到那里。主控端先发
//! 是因为 RustDesk 安装时防火墙按程序放行入站，被控端收得到；主控端主动外发，它那边不用开口子。
//!
//! 主控端每 0.1 秒发一次心跳，心跳同时是授时探测：被控端收到立即回「收到 / 回包时刻」，主控端据此对时钟
//! （[`crate::timing::ClockSync`]）。被控端 3 秒收不到心跳就认为通道断了，主控端 3 秒收不到画面同理。
//! 被控端在每帧首包的包头扩展里带上这一帧的采集 / 编码 / 发出时间，主控端据此记分段延迟。
//! 丢包不重传：主控端发现序号断档就丢掉残帧、等关键帧，并在本通道上回发「请求关键帧」（RTP 的标准做法，
//! 同 RTCP 的关键帧请求），被控端下一帧编成关键帧；[`Event::Loss`] 只告诉调用方发生了丢包。

use crate::{
    hevc::{self, DepacketizeError, Depacketizer},
    timing::{ClockEstimate, ClockSync, FrameTimes, SenderStamp, EXT_ID, EXT_OVERHEAD},
};
use bytes::Bytes;
use rand::Rng;
use rtp::{header::Header, packet::Packet};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::VecDeque,
    io::{self, ErrorKind},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use webrtc_srtp::{context::Context, protection_profile::ProtectionProfile};
use webrtc_util::marshal::{Marshal, Unmarshal};

/// 每个 UDP 包的上限：内网以太网 MTU 1500 以内留足余量，不分片。
pub const UDP_PAYLOAD: usize = 1200;
const RTP_HEADER: usize = 12;
const SRTP_GCM_TAG: usize = 16;
const RTP_PAYLOAD_MAX: usize = UDP_PAYLOAD - RTP_HEADER - SRTP_GCM_TAG;
const PT_VIDEO: u8 = 96;
const PT_CONTROL: u8 = 127;
const CTRL_HELLO: u8 = 1;
/// 主控端 → 被控端：心跳，带主控端发出时刻（授时探测）
const CTRL_KEEPALIVE: u8 = 2;
/// 被控端 → 主控端：授时回包，带心跳里的时刻和被控端收到、回包的时刻
const CTRL_TIME_ECHO: u8 = 3;
/// 主控端 → 被控端：丢了包，请下一帧编成关键帧
const CTRL_KEYFRAME: u8 = 4;
/// 心跳也是授时探测，所以比「判断对方还在」需要的密得多：授时只取最近 2 秒里最好的样本
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(100);
pub const PEER_TIMEOUT: Duration = Duration::from_secs(3);
/// 关键帧一下子几十个包，系统默认的 UDP 缓冲（Windows 上很小）收慢一点就溢出
const SOCKET_BUFFER: usize = 8 << 20;
/// 被控端收心跳的线程多久看一次「该停了」
const CONTROL_POLL: Duration = Duration::from_millis(100);
/// 等关键帧期间，隔这么久还没等到就再要一次（上一次请求或关键帧本身可能也丢了）
const LOSS_REPEAT: Duration = Duration::from_millis(200);
/// 等关键帧期间又出现新的包间隙（多半是补发的关键帧也丢了片）：最快隔这么久再要一次，
/// 免得一个关键帧的几十个分片各自触发一次请求
const LOSS_REREQUEST: Duration = Duration::from_millis(100);

/// 一次会话的密钥材料，由被控端生成、经 RustDesk 已加密的连接交给主控端。
#[derive(Clone)]
pub struct Keys {
    pub key: [u8; 16],
    pub salt: [u8; 12],
    pub token: [u8; 16],
    pub ssrc_media: u32,
    pub ssrc_control: u32,
}

impl Keys {
    pub fn generate() -> Self {
        let mut r = rand::thread_rng();
        let ssrc_media: u32 = r.gen();
        Self {
            key: r.gen(),
            salt: r.gen(),
            token: r.gen(),
            ssrc_media,
            ssrc_control: ssrc_media ^ 1,
        }
    }

    /// 被控端回授时包用的 SSRC。两个方向共用一把密钥，SRTP 的加密初始向量由 SSRC 和序号决定，
    /// 所以每条发送流必须有自己的 SSRC——同一把密钥下初始向量重复，AES-GCM 的保密和防篡改都会失效。
    fn ssrc_reply(&self) -> u32 {
        self.ssrc_media ^ 2
    }
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Srtp(String),
    /// 规定时间内没等到对方（地址不通、被防火墙挡、对方没起来）
    Timeout(&'static str),
    /// 通道建立后对方不再发包
    PeerGone,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "udp: {e}"),
            Error::Srtp(e) => write!(f, "srtp: {e}"),
            Error::Timeout(what) => write!(f, "timed out: {what}"),
            Error::PeerGone => write!(f, "the peer stopped sending for {PEER_TIMEOUT:?}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

fn srtp(keys: &Keys) -> Result<Context, Error> {
    Context::new(&keys.key, &keys.salt, ProtectionProfile::AeadAes128Gcm, None, None)
        .map_err(|e| Error::Srtp(e.to_string()))
}

fn udp(bind: SocketAddr) -> io::Result<UdpSocket> {
    let s = Socket::new(Domain::for_address(bind), Type::DGRAM, Some(Protocol::UDP))?;
    s.set_recv_buffer_size(SOCKET_BUFFER)?;
    s.set_send_buffer_size(SOCKET_BUFFER)?;
    s.bind(&bind.into())?;
    Ok(s.into())
}

fn rtp_packet(pt: u8, ssrc: u32, seq: u16, ts: u32, marker: bool, payload: Vec<u8>) -> Packet {
    Packet {
        header: Header {
            version: 2,
            payload_type: pt,
            sequence_number: seq,
            timestamp: ts,
            ssrc,
            marker,
            ..Default::default()
        },
        payload: Bytes::from(payload),
    }
}

fn seal(ctx: &mut Context, p: &Packet) -> Result<Bytes, Error> {
    let plain = p.marshal().map_err(|e| Error::Srtp(e.to_string()))?;
    ctx.encrypt_rtp(&plain).map_err(|e| Error::Srtp(e.to_string()))
}

/// 解密并解析；认证失败（不是本会话的包或被篡改）返回 None。
fn open(ctx: &mut Context, data: &[u8]) -> Option<Packet> {
    let plain = ctx.decrypt_rtp(data).ok()?;
    let mut b = &plain[..];
    Packet::unmarshal(&mut b).ok()
}

fn unspecified_like(peer: &SocketAddr) -> SocketAddr {
    match peer {
        SocketAddr::V4(_) => (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0).into(),
        SocketAddr::V6(_) => (IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0).into(),
    }
}

/// 读超时，或可以当作「这次没收到」的错误。对方端口关了时，往那边发包会招来 ICMP「端口不可达」，
/// Windows 把它变成下一次接收的 10054（连接被重置）；这类回执也可能凭空出现，不能当成致命错误——
/// 通道断没断只看 [`PEER_TIMEOUT`] 内有没有收到包。
fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused
    )
}

fn micros(d: Duration) -> i64 {
    i64::try_from(d.as_micros()).unwrap_or(i64::MAX)
}

fn micros_u32(d: Duration) -> u32 {
    u32::try_from(d.as_micros()).unwrap_or(u32::MAX)
}

fn read_i64(b: &[u8]) -> Option<i64> {
    Some(i64::from_le_bytes(b.try_into().ok()?))
}

/// 主控端发来的控制包：校验类型、SSRC、令牌，返回（种类，令牌之后的内容）。
fn control<'a>(p: &'a Packet, keys: &Keys) -> Option<(u8, &'a [u8])> {
    let head = 1 + keys.token.len();
    let ok = p.header.payload_type == PT_CONTROL
        && p.header.ssrc == keys.ssrc_control
        && p.payload.len() >= head
        && p.payload[1..head] == keys.token;
    ok.then(|| (p.payload[0], &p.payload[head..]))
}

// ------------------------------------------------------------------ 被控端

/// 被控端：开好端口，等主控端来打招呼。
pub struct Listener {
    sock: UdpSocket,
    keys: Keys,
    epoch: Instant,
}

impl Listener {
    /// `ip` 用 RustDesk 连接在被控端这一侧的地址：主控端连得到它。
    pub fn bind(ip: IpAddr, keys: Keys) -> io::Result<Self> {
        Ok(Self {
            sock: udp((ip, 0).into())?,
            keys,
            epoch: Instant::now(),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// 被控端时间轴的零点：带给主控端的时间都是从这一刻算起的微秒数。
    pub fn epoch(&self) -> Instant {
        self.epoch
    }

    pub fn accept(self, timeout: Duration) -> Result<Sender, Error> {
        let mut dec = srtp(&self.keys)?;
        let deadline = Instant::now() + timeout;
        self.sock.set_read_timeout(Some(CONTROL_POLL))?;
        let mut buf = vec![0u8; 2048];
        loop {
            if Instant::now() >= deadline {
                return Err(Error::Timeout("no hello from the controller on the pro channel"));
            }
            let (n, from) = match self.sock.recv_from(&mut buf) {
                Ok(r) => r,
                Err(e) if is_timeout(&e) => continue,
                Err(e) => return Err(e.into()),
            };
            let hello = open(&mut dec, &buf[..n]).is_some_and(|p| matches!(control(&p, &self.keys), Some((CTRL_HELLO, _))));
            if hello {
                return Sender::start(self, from, dec);
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct SendStats {
    pub frames: u64,
    pub packets: u64,
    pub bytes: u64,
}

/// 被控端一帧的时间：拿到画面、编码完成的时刻。首包发出时刻由 [`Sender::send_frame`] 自己记。
#[derive(Debug, Clone, Copy)]
pub struct Stamp {
    pub captured: Instant,
    pub encoded: Instant,
}

/// 被控端：往认准的主控端地址发画面。
///
/// 心跳由单独的线程收：收到立刻回授时包，回包里「收到时刻」才是包真正到达的时间。要是等发画面的循环
/// 每帧顺手去读，读到时可能已在缓冲里躺了大半帧，授时会被拖偏好几毫秒。
pub struct Sender {
    sock: UdpSocket,
    peer: SocketAddr,
    enc: Context,
    keys: Keys,
    seq: u16,
    epoch: Instant,
    /// 最近一次收到主控端控制包的时刻（距 `epoch` 的微秒），由收心跳的线程更新
    last_peer_us: Arc<AtomicU64>,
    /// 主控端请求了关键帧，由收心跳的线程置上
    keyframe: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    control: Option<JoinHandle<()>>,
    pub stats: SendStats,
}

impl Sender {
    fn start(listener: Listener, peer: SocketAddr, dec: Context) -> Result<Self, Error> {
        let Listener { sock, keys, epoch } = listener;
        let last_peer_us = Arc::new(AtomicU64::new(micros(epoch.elapsed()) as u64));
        let keyframe = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let ctl = Control {
            sock: sock.try_clone()?,
            peer,
            dec,
            enc: srtp(&keys)?,
            keys: keys.clone(),
            seq: rand::thread_rng().gen(),
            epoch,
            last_peer_us: last_peer_us.clone(),
            keyframe: keyframe.clone(),
            stop: stop.clone(),
        };
        let control = std::thread::Builder::new()
            .name("ddpro-control".into())
            .spawn(move || ctl.run())?;
        Ok(Self {
            sock,
            peer,
            enc: srtp(&keys)?,
            keys,
            seq: rand::thread_rng().gen(),
            epoch,
            last_peer_us,
            keyframe,
            stop,
            control: Some(control),
            stats: SendStats::default(),
        })
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// 发一帧（带起始码的 H.265 码流）。`ts` 是 90 kHz 的 RTP 时间戳；`stamp` 有值时在首包里带上
    /// 这一帧的被控端时间（重复上一帧画面等不代表新画面的帧不带）。
    pub fn send_frame(&mut self, annexb: &[u8], ts: u32, stamp: Option<Stamp>) -> Result<(), Error> {
        let mut payloads = Vec::new();
        for (k, nal) in hevc::split_annexb(annexb).into_iter().enumerate() {
            // 首包要给包头扩展留位置，整个包仍不超过 UDP_PAYLOAD
            let max = if k == 0 && stamp.is_some() {
                RTP_PAYLOAD_MAX - EXT_OVERHEAD
            } else {
                RTP_PAYLOAD_MAX
            };
            hevc::payload(nal, max, &mut payloads);
        }
        let last = payloads.len().saturating_sub(1);
        for (i, p) in payloads.into_iter().enumerate() {
            let mut pkt = rtp_packet(PT_VIDEO, self.keys.ssrc_media, self.seq, ts, i == last, p);
            if let (0, Some(s)) = (i, stamp) {
                let st = SenderStamp {
                    captured_us: micros(s.captured.saturating_duration_since(self.epoch)) as u64,
                    encode_us: micros_u32(s.encoded.saturating_duration_since(s.captured)),
                    queue_us: micros_u32(s.encoded.elapsed()),
                };
                pkt.header
                    .set_extension(EXT_ID, Bytes::copy_from_slice(&st.to_bytes()))
                    .map_err(|e| Error::Srtp(e.to_string()))?;
            }
            self.seq = self.seq.wrapping_add(1);
            let data = seal(&mut self.enc, &pkt)?;
            self.sock.send_to(&data, self.peer)?;
            self.stats.packets += 1;
            self.stats.bytes += data.len() as u64;
        }
        self.stats.frames += 1;
        Ok(())
    }

    /// 主控端是否还在（3 秒内收到过它的心跳）。
    pub fn peer_alive(&self) -> bool {
        let last = self.last_peer_us.load(Ordering::Relaxed) as i64;
        micros(self.epoch.elapsed()) - last < micros(PEER_TIMEOUT)
    }

    /// 主控端自上次查看以来请求过关键帧没有（查看即清除）。
    pub fn take_keyframe_request(&self) -> bool {
        self.keyframe.swap(false, Ordering::AcqRel)
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.control.take() {
            h.join().ok();
        }
    }
}

/// 被控端收心跳（回授时包）和关键帧请求的线程。
struct Control {
    sock: UdpSocket,
    peer: SocketAddr,
    dec: Context,
    enc: Context,
    keys: Keys,
    seq: u16,
    epoch: Instant,
    last_peer_us: Arc<AtomicU64>,
    keyframe: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Control {
    fn run(mut self) {
        let mut buf = vec![0u8; 2048];
        while !self.stop.load(Ordering::Relaxed) {
            let n = match self.sock.recv_from(&mut buf) {
                Ok((n, from)) if from == self.peer => n,
                Ok(_) => continue,
                Err(e) if is_timeout(&e) => continue,
                // 套接字坏了：不再更新心跳时刻，发送端 3 秒后按「主控端不再应答」收场
                Err(_) => return,
            };
            let t2 = micros(self.epoch.elapsed());
            let Some(p) = open(&mut self.dec, &buf[..n]) else {
                continue;
            };
            let Some((kind, body)) = control(&p, &self.keys) else {
                continue;
            };
            self.last_peer_us.store(t2 as u64, Ordering::Relaxed);
            match kind {
                CTRL_KEEPALIVE => {
                    if let Some(t1) = read_i64(body) {
                        self.echo(t1, t2);
                    }
                }
                CTRL_KEYFRAME => self.keyframe.store(true, Ordering::Release),
                _ => {}
            }
        }
    }

    fn echo(&mut self, t1: i64, t2: i64) {
        let mut payload = Vec::with_capacity(25);
        payload.push(CTRL_TIME_ECHO);
        payload.extend_from_slice(&t1.to_le_bytes());
        payload.extend_from_slice(&t2.to_le_bytes());
        payload.extend_from_slice(&micros(self.epoch.elapsed()).to_le_bytes());
        let pkt = rtp_packet(PT_CONTROL, self.keys.ssrc_reply(), self.seq, 0, false, payload);
        self.seq = self.seq.wrapping_add(1);
        // 授时回包丢了无所谓，主控端 0.1 秒后再探
        if let Ok(data) = seal(&mut self.enc, &pkt) {
            self.sock.send_to(&data, self.peer).ok();
        }
    }
}

// ------------------------------------------------------------------ 主控端

pub enum Event {
    /// 收齐的一帧（带起始码的 H.265 码流）。丢包之后，第一个收到的总是关键帧。
    /// `times` 是这一帧的分段时间，被控端没带时间（如重复上一帧画面）时为 None。
    Frame {
        data: Vec<u8>,
        timestamp: u32,
        times: Option<FrameTimes>,
    },
    /// 发现丢包。接收端已经在通道上请求了关键帧，调用方只需记录；等关键帧期间每隔 0.2 秒再请求、再报一次。
    Loss { packets: u32 },
}

#[derive(Debug, Default, Clone)]
pub struct RecvStats {
    pub frames: u64,
    pub packets: u64,
    pub bytes: u64,
    pub lost_packets: u64,
    pub loss_events: u64,
    /// 丢包后到关键帧之前收到、因为解不了而丢掉的帧
    pub discarded_frames: u64,
    /// 解密 / 认证失败的包（不是本会话的或被篡改的）
    pub auth_failures: u64,
    /// 诊断开关故意丢掉的包
    pub diag_dropped: u64,
    /// RFC 3550 到达抖动，毫秒
    pub jitter_ms: f64,
}

/// 主控端：打招呼、发心跳（兼授时）、收画面。
pub struct Receiver {
    sock: UdpSocket,
    peer: SocketAddr,
    keys: Keys,
    enc: Context,
    dec: Context,
    ctrl_seq: u16,
    last_keepalive: Instant,
    last_media: Instant,
    depack: Depacketizer,
    expect: Option<u16>,
    waiting_irap: bool,
    last_loss_event: Option<Instant>,
    pending: VecDeque<Event>,
    drop_permille: u32,
    jitter: Option<(f64, f64)>,
    start: Instant,
    clock: ClockSync,
    /// 正在收的这一帧：首包到达时刻、被控端带来的时间
    frame_first: Option<i64>,
    frame_stamp: Option<SenderStamp>,
    buf: Vec<u8>,
    pub stats: RecvStats,
}

impl Receiver {
    /// 向被控端打招呼直到收到第一个画面包。`drop_permille` 是诊断用的故意丢包比例（千分比，0 = 不丢）。
    pub fn connect(peer: SocketAddr, keys: Keys, timeout: Duration, drop_permille: u32) -> Result<Self, Error> {
        let sock = udp(unspecified_like(&peer))?;
        sock.set_read_timeout(Some(Duration::from_millis(50)))?;
        let now = Instant::now();
        let mut r = Self {
            sock,
            peer,
            enc: srtp(&keys)?,
            dec: srtp(&keys)?,
            keys,
            ctrl_seq: rand::thread_rng().gen(),
            last_keepalive: now,
            last_media: now,
            depack: Depacketizer::default(),
            expect: None,
            // 刚连上就在等关键帧（被控端第一帧就是关键帧）；记下时间，200 ms 内没等到再请求
            waiting_irap: true,
            last_loss_event: Some(now),
            pending: VecDeque::new(),
            drop_permille,
            jitter: None,
            start: now,
            clock: ClockSync::default(),
            frame_first: None,
            frame_stamp: None,
            buf: vec![0u8; 2048],
            stats: RecvStats::default(),
        };
        let deadline = now + timeout;
        let mut last_hello = None::<Instant>;
        while r.stats.packets == 0 {
            if Instant::now() >= deadline {
                return Err(Error::Timeout("no video from the controlled side on the pro channel"));
            }
            if last_hello.map_or(true, |t| t.elapsed() >= Duration::from_millis(100)) {
                r.send_control(CTRL_HELLO, &[])?;
                last_hello = Some(Instant::now());
            }
            r.poll_once()?;
        }
        r.sock.set_read_timeout(Some(Duration::from_millis(100)))?;
        Ok(r)
    }

    /// 主控端时间轴的零点：[`FrameTimes`] 里的时刻和 [`Receiver::now_us`] 都从这一刻算起。
    pub fn epoch(&self) -> Instant {
        self.start
    }

    pub fn now_us(&self) -> i64 {
        micros(self.start.elapsed())
    }

    /// 当前的对时结果（最近 2 秒里往返最快的那次）；还没有授时样本时 None。
    pub fn clock(&self) -> Option<ClockEstimate> {
        self.clock.estimate(self.now_us())
    }

    fn send_control(&mut self, kind: u8, body: &[u8]) -> Result<(), Error> {
        let mut payload = vec![kind];
        payload.extend_from_slice(&self.keys.token);
        payload.extend_from_slice(body);
        let pkt = rtp_packet(PT_CONTROL, self.keys.ssrc_control, self.ctrl_seq, 0, false, payload);
        self.ctrl_seq = self.ctrl_seq.wrapping_add(1);
        let data = seal(&mut self.enc, &pkt)?;
        self.sock.send_to(&data, self.peer)?;
        self.last_keepalive = Instant::now();
        Ok(())
    }

    /// 等下一个事件。通道断了（3 秒没画面）返回 [`Error::PeerGone`]。
    pub fn recv(&mut self) -> Result<Event, Error> {
        loop {
            if let Some(e) = self.pending.pop_front() {
                if let Event::Loss { .. } = e {
                    self.send_control(CTRL_KEYFRAME, &[])?;
                }
                return Ok(e);
            }
            if self.last_keepalive.elapsed() >= KEEPALIVE_INTERVAL {
                let t1 = self.now_us();
                self.send_control(CTRL_KEEPALIVE, &t1.to_le_bytes())?;
            }
            if self.last_media.elapsed() >= PEER_TIMEOUT {
                return Err(Error::PeerGone);
            }
            self.poll_once()?;
        }
    }

    fn poll_once(&mut self) -> Result<(), Error> {
        let (n, from) = match self.sock.recv_from(&mut self.buf) {
            Ok(r) => r,
            Err(e) if is_timeout(&e) => {
                self.repeat_loss();
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let arrival = self.now_us();
        if from != self.peer {
            return Ok(());
        }
        if self.drop_permille > 0 && rand::thread_rng().gen_range(0..1000) < self.drop_permille {
            self.stats.diag_dropped += 1;
            return Ok(());
        }
        let data = self.buf[..n].to_vec();
        let Some(p) = open(&mut self.dec, &data) else {
            self.stats.auth_failures += 1;
            return Ok(());
        };
        if p.header.payload_type == PT_CONTROL && p.header.ssrc == self.keys.ssrc_reply() {
            self.on_echo(&p.payload, arrival);
            return Ok(());
        }
        if p.header.payload_type != PT_VIDEO || p.header.ssrc != self.keys.ssrc_media {
            return Ok(());
        }
        self.last_media = Instant::now();
        self.stats.packets += 1;
        self.stats.bytes += n as u64;
        self.on_media(p, arrival);
        self.repeat_loss();
        Ok(())
    }

    fn on_echo(&mut self, payload: &[u8], t4: i64) {
        if payload.len() != 25 || payload[0] != CTRL_TIME_ECHO {
            return;
        }
        let (Some(t1), Some(t2), Some(t3)) = (read_i64(&payload[1..9]), read_i64(&payload[9..17]), read_i64(&payload[17..25])) else {
            return;
        };
        self.clock.on_echo(t1, t2, t3, t4);
    }

    fn on_media(&mut self, p: Packet, arrival: i64) {
        let seq = p.header.sequence_number;
        if let Some(expect) = self.expect {
            let gap = seq.wrapping_sub(expect);
            if gap >= 0x8000 {
                // 比预期还旧：重复或乱序到达的包，内网基本不会出现，直接丢
                return;
            }
            if gap > 0 {
                self.loss(gap as u32);
            }
        }
        self.expect = Some(seq.wrapping_add(1));
        self.frame_first.get_or_insert(arrival);
        if let Some(ext) = p.header.get_extension(EXT_ID) {
            self.frame_stamp = SenderStamp::from_bytes(&ext);
        }

        match self.depack.push(&p.payload) {
            Ok(()) => {}
            Err(DepacketizeError::BrokenFragment) => self.loss(0),
            // 只有我们自己的发送端会发到这里，它不发这些包；按丢包处理并等关键帧
            Err(DepacketizeError::Unsupported(_)) | Err(DepacketizeError::TooShort) => self.loss(0),
        }
        if !p.header.marker {
            return;
        }
        let first = self.frame_first.take().unwrap_or(arrival);
        let stamp = self.frame_stamp.take();
        let frame = match self.depack.take_frame() {
            Ok(f) => f,
            Err(_) => {
                self.loss(0);
                return;
            }
        };
        if frame.is_empty() {
            return;
        }
        if self.waiting_irap {
            if !hevc::split_annexb(&frame).into_iter().any(hevc::is_irap) {
                self.stats.discarded_frames += 1;
                return;
            }
            self.waiting_irap = false;
        }
        self.update_jitter(p.header.timestamp);
        self.stats.frames += 1;
        let offset = self.clock.estimate(arrival).map(|e| e.offset_us);
        self.pending.push_back(Event::Frame {
            data: frame,
            timestamp: p.header.timestamp,
            times: stamp.map(|s| FrameTimes::new(s, offset, first, arrival)),
        });
    }

    /// 发现丢包（`packets` 是序号间隙；0 表示分片残缺等由丢包引起的后果）。
    fn loss(&mut self, packets: u32) {
        self.stats.lost_packets += packets as u64;
        self.depack.reset();
        // 残帧的首包时间和被控端时间都作废，不能带到下一帧
        self.frame_first = None;
        self.frame_stamp = None;
        if !self.waiting_irap {
            self.waiting_irap = true;
            self.emit_loss(packets);
            return;
        }
        // 已经在等关键帧：残缺分片只是同一次丢包的后果，不再报；新的间隙限频再报
        if packets > 0 && self.last_loss_event.map_or(true, |t| t.elapsed() >= LOSS_REREQUEST) {
            self.emit_loss(packets);
        }
    }

    fn emit_loss(&mut self, packets: u32) {
        self.stats.loss_events += 1;
        self.last_loss_event = Some(Instant::now());
        self.pending.push_back(Event::Loss { packets });
    }

    /// 等关键帧太久（请求或关键帧本身也丢了，或刚连上时错过了第一个关键帧）就再要一次。
    fn repeat_loss(&mut self) {
        let due = self.last_loss_event.map_or(true, |t| t.elapsed() >= LOSS_REPEAT);
        if self.waiting_irap && due {
            self.emit_loss(0);
        }
    }

    fn update_jitter(&mut self, ts: u32) {
        // RFC 3550：按 90 kHz 时钟比较「到达间隔」和「发送时间戳间隔」的差
        let arrival = self.start.elapsed().as_secs_f64() * 90_000.0;
        let transit = arrival - ts as f64;
        if let Some((prev, j)) = self.jitter {
            let d = (transit - prev).abs();
            let j = j + (d - j) / 16.0;
            self.jitter = Some((transit, j));
            self.stats.jitter_ms = j / 90.0;
        } else {
            self.jitter = Some((transit, 0.0));
        }
    }
}
