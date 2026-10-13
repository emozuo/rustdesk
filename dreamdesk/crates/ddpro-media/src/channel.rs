//! 专业画面通道：UDP 直连 + 标准 RTP + 标准 SRTP（AES-128-GCM，被篡改的包直接丢弃）。
//!
//! 建立方式：被控端在 RustDesk 已加密的连接里把自己的地址、一次性令牌和密钥告诉主控端；主控端先往被控端
//! 发「打招呼」包（带令牌、同样加密），被控端认准第一个带对令牌的来源地址，之后把画面发到那里。主控端先发
//! 是因为 RustDesk 安装时防火墙按程序放行入站，被控端收得到；主控端主动外发，它那边不用开口子。
//!
//! 主控端每半秒发一次心跳；被控端 3 秒收不到心跳就认为通道断了，主控端 3 秒收不到画面同理。
//! 丢包不重传：主控端发现序号断档就丢掉残帧、等关键帧，并通过 [`Event::Loss`] 让调用方经 RustDesk
//! 原连接请求关键帧。

use crate::hevc::{self, DepacketizeError, Depacketizer};
use bytes::Bytes;
use rand::Rng;
use rtp::{header::Header, packet::Packet};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::VecDeque,
    io::{self, ErrorKind},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
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
const CTRL_KEEPALIVE: u8 = 2;
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(500);
pub const PEER_TIMEOUT: Duration = Duration::from_secs(3);
/// 关键帧一下子几十个包，系统默认的 UDP 缓冲（Windows 上很小）收慢一点就溢出
const SOCKET_BUFFER: usize = 8 << 20;
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

// ------------------------------------------------------------------ 被控端

/// 被控端：开好端口，等主控端来打招呼。
pub struct Listener {
    sock: UdpSocket,
    keys: Keys,
}

impl Listener {
    /// `ip` 用 RustDesk 连接在被控端这一侧的地址：主控端连得到它。
    pub fn bind(ip: IpAddr, keys: Keys) -> io::Result<Self> {
        Ok(Self {
            sock: udp((ip, 0).into())?,
            keys,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    pub fn accept(self, timeout: Duration) -> Result<Sender, Error> {
        let mut dec = srtp(&self.keys)?;
        let deadline = Instant::now() + timeout;
        self.sock.set_read_timeout(Some(Duration::from_millis(100)))?;
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
            if let Some(p) = open(&mut dec, &buf[..n]) {
                if control(&p, &self.keys) == Some(CTRL_HELLO) {
                    self.sock.set_nonblocking(true)?;
                    return Ok(Sender {
                        sock: self.sock,
                        peer: from,
                        enc: srtp(&self.keys)?,
                        dec,
                        keys: self.keys,
                        seq: rand::thread_rng().gen(),
                        last_peer: Instant::now(),
                        buf,
                        stats: SendStats::default(),
                    });
                }
            }
        }
    }
}

fn control(p: &Packet, keys: &Keys) -> Option<u8> {
    let ok = p.header.payload_type == PT_CONTROL
        && p.header.ssrc == keys.ssrc_control
        && p.payload.len() == 1 + keys.token.len()
        && p.payload[1..] == keys.token;
    ok.then(|| p.payload[0])
}

#[derive(Debug, Default, Clone)]
pub struct SendStats {
    pub frames: u64,
    pub packets: u64,
    pub bytes: u64,
}

/// 被控端：往认准的主控端地址发画面。
pub struct Sender {
    sock: UdpSocket,
    peer: SocketAddr,
    enc: Context,
    dec: Context,
    keys: Keys,
    seq: u16,
    last_peer: Instant,
    buf: Vec<u8>,
    pub stats: SendStats,
}

impl Sender {
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// 发一帧（带起始码的 H.265 码流）。`ts` 是 90 kHz 的 RTP 时间戳。
    pub fn send_frame(&mut self, annexb: &[u8], ts: u32) -> Result<(), Error> {
        let mut payloads = Vec::new();
        for nal in hevc::split_annexb(annexb) {
            hevc::payload(nal, RTP_PAYLOAD_MAX, &mut payloads);
        }
        let last = payloads.len().saturating_sub(1);
        for (i, p) in payloads.into_iter().enumerate() {
            let pkt = rtp_packet(PT_VIDEO, self.keys.ssrc_media, self.seq, ts, i == last, p);
            self.seq = self.seq.wrapping_add(1);
            let data = seal(&mut self.enc, &pkt)?;
            self.send(&data)?;
            self.stats.packets += 1;
            self.stats.bytes += data.len() as u64;
        }
        self.stats.frames += 1;
        Ok(())
    }

    fn send(&self, data: &[u8]) -> Result<(), Error> {
        // 非阻塞套接字（为了不阻塞地收心跳）：发送缓冲满时稍等再试，不能把包丢在本机
        let start = Instant::now();
        loop {
            match self.sock.send_to(data, self.peer) {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == ErrorKind::WouldBlock && start.elapsed() < Duration::from_millis(50) => {
                    std::thread::sleep(Duration::from_micros(200));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// 收掉已到的心跳，返回主控端是否还在（3 秒内收到过心跳）。
    pub fn peer_alive(&mut self) -> bool {
        loop {
            match self.sock.recv_from(&mut self.buf) {
                Ok((n, from)) if from == self.peer => {
                    if let Some(p) = open(&mut self.dec, &self.buf[..n]) {
                        if control(&p, &self.keys).is_some() {
                            self.last_peer = Instant::now();
                        }
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        self.last_peer.elapsed() < PEER_TIMEOUT
    }
}

// ------------------------------------------------------------------ 主控端

pub enum Event {
    /// 收齐的一帧（带起始码的 H.265 码流）。丢包之后，第一个收到的总是关键帧。
    Frame { data: Vec<u8>, timestamp: u32 },
    /// 发现丢包：调用方应经 RustDesk 原连接请求关键帧。等关键帧期间每隔 0.5 秒会再报一次。
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

/// 主控端：打招呼、发心跳、收画面。
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
                r.send_control(CTRL_HELLO)?;
                last_hello = Some(Instant::now());
            }
            r.poll_once()?;
        }
        r.sock.set_read_timeout(Some(Duration::from_millis(100)))?;
        Ok(r)
    }

    fn send_control(&mut self, kind: u8) -> Result<(), Error> {
        let mut payload = vec![kind];
        payload.extend_from_slice(&self.keys.token);
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
                return Ok(e);
            }
            if self.last_keepalive.elapsed() >= KEEPALIVE_INTERVAL {
                self.send_control(CTRL_KEEPALIVE)?;
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
        if p.header.payload_type != PT_VIDEO || p.header.ssrc != self.keys.ssrc_media {
            return Ok(());
        }
        self.last_media = Instant::now();
        self.stats.packets += 1;
        self.stats.bytes += n as u64;
        self.on_media(p);
        self.repeat_loss();
        Ok(())
    }

    fn on_media(&mut self, p: Packet) {
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

        match self.depack.push(&p.payload) {
            Ok(()) => {}
            Err(DepacketizeError::BrokenFragment) => self.loss(0),
            // 只有我们自己的发送端会发到这里，它不发这些包；按丢包处理并等关键帧
            Err(DepacketizeError::Unsupported(_)) | Err(DepacketizeError::TooShort) => self.loss(0),
        }
        if !p.header.marker {
            return;
        }
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
        self.pending.push_back(Event::Frame {
            data: frame,
            timestamp: p.header.timestamp,
        });
    }

    /// 发现丢包（`packets` 是序号间隙；0 表示分片残缺等由丢包引起的后果）。
    fn loss(&mut self, packets: u32) {
        self.stats.lost_packets += packets as u64;
        self.depack.reset();
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
