//! 专业画面通道：UDP 直连 + 标准 RTP + 标准 SRTP（AES-128-GCM，被篡改的包直接丢弃）。
//!
//! 建立方式：被控端在 RustDesk 已加密的连接里把自己的地址、一次性令牌和密钥告诉主控端；主控端先往被控端
//! 发「打招呼」包（带令牌、同样加密），被控端认准第一个带对令牌的来源地址，之后把画面发到那里。主控端先发
//! 是因为 RustDesk 安装时防火墙按程序放行入站，被控端收得到；主控端主动外发，它那边不用开口子。
//!
//! 主控端每 0.1 秒发一次心跳，心跳同时是授时探测：被控端收到立即回「收到 / 回包时刻」，主控端据此对时钟
//! （[`crate::timing::ClockSync`]）。被控端 3 秒收不到心跳就认为通道断了，主控端 3 秒收不到画面同理。
//! 被控端在每帧首包的包头扩展里带上这一帧的采集 / 编码 / 发出时间，主控端据此记分段延迟。
//!
//! 抗丢包按 WebRTC 在低往返时延下的做法（内网往返不到 1 毫秒）：
//! - 丢包重传：主控端发现序号断档立即回报缺哪几个包（同 RTCP 的通用丢包回报），被控端从最近发过的包里补发，
//!   补发包按 RFC 4588 包装（自己的 SSRC 和序号，载荷前两字节是原序号），所以不会和原包共用加密初始向量；
//!   主控端先把断档后面的包按序号存着，补齐了再照常拼帧。
//! - 补发也等不到（[`NACK_GIVE_UP`]）才丢掉残帧、等关键帧，并在本通道上回发「请求关键帧」（同 RTCP 的
//!   关键帧请求），被控端下一帧编成关键帧；[`Event::Loss`] 只告诉调用方发生了这种救不回来的丢包。
//! - 发包节奏：一帧预算以内的量立即发（普通帧不加延迟），超出的部分（关键帧）按平均码率的几倍匀速发，
//!   免得上千个包一口气冲爆交换机 / 网卡缓冲（[`Sender::pace`]）。

use crate::{
    hevc::{self, DepacketizeError, Depacketizer},
    timing::{ClockEstimate, ClockSync, FrameTimes, SenderStamp, EXT_ID, EXT_OVERHEAD},
};
use bytes::Bytes;
use rand::Rng;
use rtp::{header::Header, packet::Packet};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::{HashMap, VecDeque},
    io::{self, ErrorKind},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
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
/// 补发包载荷前多出的原序号
const RTX_OSN: usize = 2;
/// 给补发包留出原序号的位置：满长的包补发时也不超过 UDP_PAYLOAD
const RTP_PAYLOAD_MAX: usize = UDP_PAYLOAD - RTP_HEADER - SRTP_GCM_TAG - RTX_OSN;
const PT_VIDEO: u8 = 96;
/// 补发包（RFC 4588）
const PT_RTX: u8 = 97;
const PT_CONTROL: u8 = 127;
const CTRL_HELLO: u8 = 1;
/// 主控端 → 被控端：心跳，带主控端发出时刻（授时探测）
const CTRL_KEEPALIVE: u8 = 2;
/// 被控端 → 主控端：授时回包，带心跳里的时刻和被控端收到、回包的时刻
const CTRL_TIME_ECHO: u8 = 3;
/// 主控端 → 被控端：丢了包，请下一帧编成关键帧
const CTRL_KEYFRAME: u8 = 4;
/// 主控端 → 被控端：专业画面不要了（用户关了专业窗口），被控端不用等心跳超时
const CTRL_BYE: u8 = 5;
/// 主控端 → 被控端：缺这些序号的画面包，请补发（内容是若干个小端 u16 序号）
const CTRL_NACK: u8 = 6;
/// 一个补发请求里最多列多少个序号（控制包不超过 UDP_PAYLOAD）
const NACK_PER_PACKET: usize = 256;
/// 被控端留多少个最近发过的包备补发：要覆盖主控端放弃等待（[`NACK_GIVE_UP`]）之前发出的所有包。
/// 30 Mbps 下 40 毫秒只有约 130 个包，但关键帧上千个包是一口气交给发包节奏的，留够一个关键帧
const HISTORY: usize = 2048;
/// 发包节奏：超出一帧预算的部分按平均码率的这么多倍匀速发（WebRTC 的默认倍数）
const PACE_FACTOR: f64 = 2.5;
/// 欠账超过这么多字节才睡：逐包睡的话，睡眠精度远粗于一个包的发送间隔
const PACE_SLACK: f64 = (4 * UDP_PAYLOAD) as f64;
/// 主控端：缺的包隔这么久还没补到就再要一次（补发请求或补发包本身也可能丢）。内网往返不到 1 毫秒，
/// 5 毫秒还没到基本就是丢了
const NACK_RETRY: Duration = Duration::from_millis(5);
/// 每个缺的包最多要几次（都在 [`NACK_GIVE_UP`] 之内）
const NACK_TRIES: u8 = 3;
/// 主控端：缺的包最多等这么久，过了按救不回来的丢包处理（丢残帧、请求关键帧）
const NACK_GIVE_UP: Duration = Duration::from_millis(40);
/// 有缺包在等补发、或一帧还没收齐时，收包超时缩短到这么多，好按时再要 / 放弃 / 试探
const NACK_POLL: Duration = Duration::from_millis(2);
/// 一帧还没收齐、这么久没来新包：多半是这帧末尾的包丢了（后面没有更大的序号来暴露断档），
/// 试探着请求下一个序号；不试探就要等下一帧（16.7 毫秒）才发现
const TAIL_PROBE: Duration = Duration::from_millis(3);
/// 缺包的检查（再要 / 放弃）最多这么频繁：关键帧成片丢包时缺包表上千项，不必每收一个包都扫一遍
const GAP_CHECK: Duration = Duration::from_millis(1);
/// 心跳也是授时探测，所以比「判断对方还在」需要的密得多：授时只取最近 2 秒里最好的样本
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(100);
pub const PEER_TIMEOUT: Duration = Duration::from_secs(3);
/// 关键帧一下子几十个包，系统默认的 UDP 缓冲（Windows 上很小）收慢一点就溢出
const SOCKET_BUFFER: usize = 8 << 20;
/// 被控端收心跳的线程多久看一次「该停了」
const CONTROL_POLL: Duration = Duration::from_millis(100);
/// 等关键帧期间，隔这么久还没等到就再要一次（上一次请求或关键帧本身可能也丢了）
const LOSS_REPEAT: Duration = Duration::from_millis(200);
/// 等关键帧期间又出现救不回来的丢包（多半是被控端应请求发来的关键帧也丢了片）：最快隔这么久再要一次，
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

    /// 被控端补发包用的 SSRC（理由同上）。
    fn ssrc_rtx(&self) -> u32 {
        self.ssrc_media ^ 3
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

/// RFC 4588 补发包：原包的时间戳、结束标记和包头扩展照搬，载荷前加两字节原序号（网络字节序）。
fn rtx_packet(orig: &Packet, ssrc: u32, seq: u16) -> Result<Packet, Error> {
    let mut payload = Vec::with_capacity(RTX_OSN + orig.payload.len());
    payload.extend_from_slice(&orig.header.sequence_number.to_be_bytes());
    payload.extend_from_slice(&orig.payload);
    let mut p = rtp_packet(PT_RTX, ssrc, seq, orig.header.timestamp, orig.header.marker, payload);
    if let Some(ext) = orig.header.get_extension(EXT_ID) {
        p.header.set_extension(EXT_ID, ext).map_err(|e| Error::Srtp(e.to_string()))?;
    }
    Ok(p)
}

/// 补发包还原成原来的画面包。
fn rtx_unwrap(rtx: Packet, keys: &Keys) -> Option<Packet> {
    if rtx.payload.len() < RTX_OSN {
        return None;
    }
    let mut header = rtx.header;
    header.payload_type = PT_VIDEO;
    header.ssrc = keys.ssrc_media;
    header.sequence_number = u16::from_be_bytes([rtx.payload[0], rtx.payload[1]]);
    Some(Packet {
        header,
        payload: rtx.payload.slice(RTX_OSN..),
    })
}

/// 被控端最近发过的画面包（加密前），按序号取出来补发。
struct History(Vec<Option<Packet>>);

impl History {
    fn new() -> Self {
        Self(vec![None; HISTORY])
    }

    fn put(&mut self, p: Packet) {
        let i = p.header.sequence_number as usize % HISTORY;
        self.0[i] = Some(p);
    }

    fn get(&self, seq: u16) -> Option<Packet> {
        self.0[seq as usize % HISTORY]
            .as_ref()
            .filter(|p| p.header.sequence_number == seq)
            .cloned()
    }
}

/// 发包节奏（令牌桶）：桶容量是一帧的预算，按平均码率的 [`PACE_FACTOR`] 倍补充；普通帧桶里总够，立即发完，
/// 关键帧这种超出预算的大帧，超出部分按补充速度匀速发。
struct Pacer {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl Pacer {
    fn new(kbps: u32, fps: u32) -> Self {
        let bytes_per_sec = kbps as f64 * 1000.0 / 8.0;
        let burst = bytes_per_sec / fps.max(1) as f64;
        Self {
            rate: bytes_per_sec * PACE_FACTOR,
            burst,
            tokens: burst,
            last: Instant::now(),
        }
    }

    fn take(&mut self, len: usize) {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * self.rate).min(self.burst);
        self.last = now;
        self.tokens -= len as f64;
        if self.tokens < -PACE_SLACK {
            std::thread::sleep(Duration::from_secs_f64(-self.tokens / self.rate));
        }
    }
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
    /// 应主控端请求补发的包（补发在收控制包的线程里做，每发一帧同步一次计数）
    pub retransmitted: u64,
    /// 想补发却没发出去的包：已经不在历史里（太旧）、或加密 / 发送失败
    pub retransmit_failed: u64,
}

/// 收控制包的线程和发画面的线程共用的补发状态。
#[derive(Default)]
struct Rtx {
    sent: AtomicU64,
    failed: AtomicU64,
    /// 发包节奏参数，`kbps << 32 | fps`；0 = 不控节奏。补发也按同样的速度发，免得成批补发又冲爆缓冲
    pace: AtomicU64,
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
    /// 主控端说不要了
    bye: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    control: Option<JoinHandle<()>>,
    /// 最近发过的包，收控制包的线程按主控端的请求从这里取出来补发
    history: Arc<Mutex<History>>,
    rtx: Arc<Rtx>,
    pacer: Option<Pacer>,
    pub stats: SendStats,
}

impl Sender {
    fn start(listener: Listener, peer: SocketAddr, dec: Context) -> Result<Self, Error> {
        let Listener { sock, keys, epoch } = listener;
        let last_peer_us = Arc::new(AtomicU64::new(micros(epoch.elapsed()) as u64));
        let keyframe = Arc::new(AtomicBool::new(false));
        let bye = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let history = Arc::new(Mutex::new(History::new()));
        let rtx = Arc::new(Rtx::default());
        let ctl = Control {
            sock: sock.try_clone()?,
            peer,
            dec,
            enc: srtp(&keys)?,
            keys: keys.clone(),
            seq: rand::thread_rng().gen(),
            rtx_seq: rand::thread_rng().gen(),
            epoch,
            last_peer_us: last_peer_us.clone(),
            keyframe: keyframe.clone(),
            bye: bye.clone(),
            stop: stop.clone(),
            history: history.clone(),
            rtx: rtx.clone(),
            pacer: None,
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
            bye,
            stop,
            control: Some(control),
            history,
            rtx,
            pacer: None,
            stats: SendStats::default(),
        })
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// 打开发包节奏：`kbps` / `fps` 是编码器的平均码率和帧率；码率为 0 或不调就一口气发（自测用）。
    pub fn pace(&mut self, kbps: u32, fps: u32) {
        let on = kbps > 0;
        self.pacer = on.then(|| Pacer::new(kbps, fps));
        let packed = if on { (kbps as u64) << 32 | fps as u64 } else { 0 };
        self.rtx.pace.store(packed, Ordering::Relaxed);
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
            self.history.lock().unwrap().put(pkt);
            if let Some(p) = self.pacer.as_mut() {
                p.take(data.len());
            }
            self.sock.send_to(&data, self.peer)?;
            self.stats.packets += 1;
            self.stats.bytes += data.len() as u64;
        }
        self.stats.frames += 1;
        self.stats.retransmitted = self.rtx.sent.load(Ordering::Relaxed);
        self.stats.retransmit_failed = self.rtx.failed.load(Ordering::Relaxed);
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

    /// 主控端说专业画面不要了（关了专业窗口）。
    pub fn peer_said_bye(&self) -> bool {
        self.bye.load(Ordering::Acquire)
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

/// 被控端收心跳（回授时包）、补发请求和关键帧请求的线程。补发也在这里做：收到就发，不等发画面的循环。
struct Control {
    sock: UdpSocket,
    peer: SocketAddr,
    dec: Context,
    enc: Context,
    keys: Keys,
    seq: u16,
    rtx_seq: u16,
    epoch: Instant,
    last_peer_us: Arc<AtomicU64>,
    keyframe: Arc<AtomicBool>,
    bye: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    history: Arc<Mutex<History>>,
    rtx: Arc<Rtx>,
    /// 补发用的发包节奏，和 [`Rtx::pace`] 里的参数对应
    pacer: Option<(u64, Pacer)>,
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
                CTRL_BYE => self.bye.store(true, Ordering::Release),
                CTRL_NACK => self.retransmit(body),
                _ => {}
            }
        }
    }

    /// 不在历史里的（太旧被新包覆盖了，或主控端试探的那个序号还没发出）补不了，主控端等不到会按丢包处理；
    /// 补不了和发送失败都只计数——这个线程没有别的出口，主控端那边的放弃计时兜底。
    fn retransmit(&mut self, body: &[u8]) {
        let asked = body.len() / 2;
        let wanted: Vec<Packet> = {
            let history = self.history.lock().unwrap();
            body.chunks_exact(2)
                .filter_map(|b| history.get(u16::from_le_bytes([b[0], b[1]])))
                .collect()
        };
        let mut failed = asked - wanted.len();
        let pace = self.rtx.pace.load(Ordering::Relaxed);
        if self.pacer.as_ref().map(|(p, _)| *p) != Some(pace) {
            self.pacer = (pace != 0).then(|| (pace, Pacer::new((pace >> 32) as u32, pace as u32)));
        }
        for orig in wanted {
            let sent = rtx_packet(&orig, self.keys.ssrc_rtx(), self.rtx_seq).and_then(|pkt| {
                self.rtx_seq = self.rtx_seq.wrapping_add(1);
                let data = seal(&mut self.enc, &pkt)?;
                if let Some((_, p)) = self.pacer.as_mut() {
                    p.take(data.len());
                }
                self.sock.send_to(&data, self.peer)?;
                Ok(())
            });
            match sent {
                Ok(()) => {
                    self.rtx.sent.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => failed += 1,
            }
        }
        self.rtx.failed.fetch_add(failed as u64, Ordering::Relaxed);
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
    /// 救不回来的丢包（补发也没等到）。接收端已经在通道上请求了关键帧，调用方只需记录；
    /// 等关键帧期间每隔 0.2 秒再请求、再报一次（关键帧的包已经开始到了的话不算等太久）。
    /// 补发补回来的丢包不报。
    Loss { packets: u32 },
}

#[derive(Debug, Default, Clone)]
pub struct RecvStats {
    pub frames: u64,
    pub packets: u64,
    pub bytes: u64,
    /// 救不回来的丢包（补发也没等到）
    pub lost_packets: u64,
    pub loss_events: u64,
    /// 发现缺了、请求过补发的包
    pub nacked: u64,
    /// 缺了之后又补齐的包（补发到了，或原包晚到）
    pub recovered: u64,
    /// 丢包后到关键帧之前收到、因为解不了而丢掉的帧
    pub discarded_frames: u64,
    /// 解密 / 认证失败的包（不是本会话的或被篡改的）
    pub auth_failures: u64,
    /// 诊断开关故意丢掉的包
    pub diag_dropped: u64,
    /// RFC 3550 到达抖动，毫秒
    pub jitter_ms: f64,
}

struct Gap {
    since: Instant,
    asked: Instant,
    tries: u8,
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
    /// 下一个该拼进帧的序号
    expect: Option<u16>,
    /// 断档后面先到的包（连同到达时刻），等缺的补齐再按序拼
    held: HashMap<u16, (Packet, i64)>,
    /// 缺的包：发现时刻、上次请求时刻、请求次数
    missing: HashMap<u16, Gap>,
    /// 收到过（或试探请求过）的最大序号，缺包表不空时才有意义
    highest: u16,
    /// 没有缺包、也没有收了一半的帧时的收包超时；否则缩到 [`NACK_POLL`]
    idle_timeout: Duration,
    read_timeout: Duration,
    /// 下一次检查缺包（再要 / 放弃 / 试探）的时刻
    next_gap_check: Instant,
    waiting_irap: bool,
    /// 等关键帧期间，关键帧的包已经开始到了（还没到齐）
    keyframe_arriving: bool,
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
        let idle_timeout = Duration::from_millis(50);
        sock.set_read_timeout(Some(idle_timeout))?;
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
            held: HashMap::new(),
            missing: HashMap::new(),
            highest: 0,
            idle_timeout,
            read_timeout: idle_timeout,
            next_gap_check: now,
            // 刚连上就在等关键帧（被控端第一帧就是关键帧）；记下时间，200 ms 内没等到再请求
            waiting_irap: true,
            keyframe_arriving: false,
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
        r.idle_timeout = Duration::from_millis(100);
        Ok(r)
    }

    /// 主控端时间轴的零点：[`FrameTimes`] 里的时刻和 [`Receiver::now_us`] 都从这一刻算起。
    pub fn epoch(&self) -> Instant {
        self.start
    }

    pub fn now_us(&self) -> i64 {
        micros(self.start.elapsed())
    }

    /// 告诉被控端专业画面不要了（关专业窗口时调），它立即退回普通画面。连发三次：控制包丢了也不至于让它
    /// 白等 3 秒的心跳超时。
    pub fn bye(&mut self) -> Result<(), Error> {
        for _ in 0..3 {
            self.send_control(CTRL_BYE, &[])?;
        }
        Ok(())
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
            // 只按心跳自己计时：补发请求再密也不能顶掉心跳，心跳同时是授时探测
            if self.last_keepalive.elapsed() >= KEEPALIVE_INTERVAL {
                let t1 = self.now_us();
                self.send_control(CTRL_KEEPALIVE, &t1.to_le_bytes())?;
                self.last_keepalive = Instant::now();
            }
            if self.last_media.elapsed() >= PEER_TIMEOUT {
                return Err(Error::PeerGone);
            }
            self.poll_once()?;
        }
    }

    fn poll_once(&mut self) -> Result<(), Error> {
        let timeout = if self.missing.is_empty() && self.frame_first.is_none() {
            self.idle_timeout
        } else {
            NACK_POLL
        };
        if timeout != self.read_timeout {
            self.sock.set_read_timeout(Some(timeout))?;
            self.read_timeout = timeout;
        }
        let (n, from) = match self.sock.recv_from(&mut self.buf) {
            Ok(r) => r,
            Err(e) if is_timeout(&e) => {
                self.check_gaps()?;
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
        let p = match (p.header.payload_type, p.header.ssrc) {
            (PT_VIDEO, ssrc) if ssrc == self.keys.ssrc_media => p,
            (PT_RTX, ssrc) if ssrc == self.keys.ssrc_rtx() => match rtx_unwrap(p, &self.keys) {
                Some(p) => p,
                None => return Ok(()),
            },
            _ => return Ok(()),
        };
        self.last_media = Instant::now();
        self.stats.packets += 1;
        self.stats.bytes += n as u64;
        self.on_media(p, arrival)?;
        self.check_gaps()?;
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

    /// 按序号排好再拼帧：正好是下一个就直接拼；跳号了就先存着、请求补发缺的。
    fn on_media(&mut self, p: Packet, arrival: i64) -> Result<(), Error> {
        let seq = p.header.sequence_number;
        let Some(expect) = self.expect else {
            self.expect = Some(seq.wrapping_add(1));
            self.feed(p, arrival);
            return Ok(());
        };
        let ahead = seq.wrapping_sub(expect);
        if ahead >= 0x8000 {
            // 比该拼的还旧：重复的，或已经放弃等待的包晚到了
            return Ok(());
        }
        if self.missing.remove(&seq).is_some() {
            self.stats.recovered += 1;
        }
        if ahead == 0 {
            self.feed(p, arrival);
            self.expect = Some(seq.wrapping_add(1));
            self.drain();
            return Ok(());
        }
        if ahead as usize >= HISTORY {
            // 断档比被控端留的历史还长，补不回来了：缺的全按丢包算（暂存着的不算丢，但也只能扔掉），从这个包重新开始
            self.give_up(ahead as u32 - self.held.len() as u32);
            self.expect = Some(seq.wrapping_add(1));
            self.feed(p, arrival);
            return Ok(());
        }
        if self.held.contains_key(&seq) {
            return Ok(());
        }
        // 新出现的缺口：上次见过的最大序号之后、这个包之前（缺包表空着时暂存表必然也空）
        let from = if self.missing.is_empty() {
            expect
        } else {
            self.highest.wrapping_add(1)
        };
        let fresh = seq.wrapping_sub(from);
        let mut nack = Vec::new();
        if fresh < 0x8000 {
            let now = Instant::now();
            for k in 0..fresh {
                let s = from.wrapping_add(k);
                self.missing.insert(
                    s,
                    Gap {
                        since: now,
                        asked: now,
                        tries: 1,
                    },
                );
                nack.push(s);
            }
            self.highest = seq;
        }
        self.held.insert(seq, (p, arrival));
        self.stats.nacked += nack.len() as u64;
        self.send_nack(&nack)
    }

    /// 补齐了的就接着往下拼。
    fn drain(&mut self) {
        while let Some(expect) = self.expect {
            let Some((p, arrival)) = self.held.remove(&expect) else {
                break;
            };
            self.feed(p, arrival);
            self.expect = Some(expect.wrapping_add(1));
        }
    }

    /// 到时间还没补到的再要一次；最老的缺包等过了 [`NACK_GIVE_UP`] 就放弃，跳过这一段缺口接着拼
    /// （拼帧那边会丢掉残帧、等关键帧）。一帧收了一半、好一会儿没新包，就试探着要下一个序号。
    fn check_gaps(&mut self) -> Result<(), Error> {
        let now = Instant::now();
        if now < self.next_gap_check {
            return Ok(());
        }
        self.next_gap_check = now + GAP_CHECK;
        if let Some(expect) = self.expect {
            let stalled = self.frame_first.is_some() && self.last_media.elapsed() >= TAIL_PROBE;
            if stalled && self.missing.is_empty() {
                self.missing.insert(
                    expect,
                    Gap {
                        since: now,
                        asked: now,
                        tries: 1,
                    },
                );
                self.highest = expect;
                self.stats.nacked += 1;
                self.send_nack(&[expect])?;
            }
        }
        if self.missing.is_empty() {
            return Ok(());
        }
        let mut again = Vec::new();
        for (&s, g) in self.missing.iter_mut() {
            if g.tries < NACK_TRIES && now.duration_since(g.asked) >= NACK_RETRY {
                g.asked = now;
                g.tries += 1;
                again.push(s);
            }
        }
        self.send_nack(&again)?;
        while let Some(expect) = self.expect {
            if !self.missing.get(&expect).is_some_and(|g| now.duration_since(g.since) >= NACK_GIVE_UP) {
                break;
            }
            let mut s = expect;
            let mut skipped = 0;
            while self.missing.remove(&s).is_some() {
                s = s.wrapping_add(1);
                skipped += 1;
            }
            self.expect = Some(s);
            self.loss(skipped);
            self.drain();
        }
        Ok(())
    }

    /// 放弃所有在等的缺包（断档太长时）。
    fn give_up(&mut self, lost: u32) {
        self.missing.clear();
        self.held.clear();
        self.loss(lost);
    }

    fn send_nack(&mut self, seqs: &[u16]) -> Result<(), Error> {
        for chunk in seqs.chunks(NACK_PER_PACKET) {
            let body: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
            self.send_control(CTRL_NACK, &body)?;
        }
        Ok(())
    }

    /// `arrival` 是这个包到达的时刻。暂存后才拼进来的包到得比补发包早，所以一帧的首包到达取拼进来的包里最早的，
    /// 收齐时刻取末包拼进来的此刻（等补发的时间要算在帧内跨度里）。
    fn feed(&mut self, p: Packet, arrival: i64) {
        self.frame_first = Some(self.frame_first.map_or(arrival, |f| f.min(arrival)));
        if let Some(ext) = p.header.get_extension(EXT_ID) {
            self.frame_stamp = SenderStamp::from_bytes(&ext);
        }
        if self.waiting_irap && hevc::payload_starts_keyframe(&p.payload) {
            self.keyframe_arriving = true;
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
        self.keyframe_arriving = false;
        let done = self.now_us().max(arrival);
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
        let offset = self.clock.estimate(done).map(|e| e.offset_us);
        self.pending.push_back(Event::Frame {
            data: frame,
            timestamp: p.header.timestamp,
            times: stamp.map(|s| FrameTimes::new(s, offset, first, done)),
        });
    }

    /// 发现丢包（`packets` 是序号间隙；0 表示分片残缺等由丢包引起的后果）。
    fn loss(&mut self, packets: u32) {
        self.stats.lost_packets += packets as u64;
        self.depack.reset();
        self.keyframe_arriving = false;
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

    /// 等关键帧太久（请求丢了，或刚连上时错过了第一个关键帧）就再要一次。关键帧正在到达时不算：
    /// 发包节奏把大关键帧摊开发，到齐可能要上百毫秒；它要是到残了，按丢包另行请求。
    fn repeat_loss(&mut self) {
        let due = self.last_loss_event.map_or(true, |t| t.elapsed() >= LOSS_REPEAT);
        if self.waiting_irap && due && !self.keyframe_arriving {
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
