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
//! - 补发也等不到（40 毫秒）才丢掉残帧、等关键帧，并在本通道上回发「请求关键帧」（同 RTCP 的
//!   关键帧请求），被控端下一帧编成关键帧；[`Event::Loss`] 只告诉调用方发生了这种救不回来的丢包。
//! - 发包节奏：一帧预算以内的量立即发（普通帧不加延迟），超出的部分（关键帧）按平均码率的几倍匀速发，
//!   免得上千个包一口气冲爆交换机 / 网卡缓冲（[`Sender::pace`]）。
//!
//! 静止无损（P1 第 7 项，协议 v8 起）：同一条流、同一套序号里还有「方块消息」（载荷类型 [`PT_TILE`]），排序、丢包补发、
//! 加密都和画面包共用；补发按 RFC 4588 给它单独的补发载荷类型，补回来知道原来是哪种。两种消息：
//! - 「这一帧哪些方块变了」：紧贴在那一帧的画面包前面发，接收端挂在那一帧上交出去（[`Event::Frame`] 的 `changed`）；
//! - 一个方块的无损数据（[`crate::tile`] 编好的）：插在两帧之间发，带它取自哪一帧的时间戳（[`Event::Tile`]）。
//!
//! 一条消息占连续的若干个包，首包载荷第一个字节标出「消息开头」，末包打结束标记。消息只在两帧之间发，不会插进一帧中间。

use crate::timing::EXT_ID;
use bytes::Bytes;
use rand::Rng;
use rtp::{header::Header, packet::Packet};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io::{self, ErrorKind},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    time::Duration,
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
/// 方块消息（和画面包同一 SSRC、同一套序号）
const PT_TILE: u8 = 98;
/// 方块消息的补发包（RFC 4588：每种原载荷类型一个补发载荷类型）
const PT_RTX_TILE: u8 = 99;
const PT_CONTROL: u8 = 127;
/// 方块消息每个包载荷的第一个字节：这一位表示「消息从这个包开始」
const TILE_FIRST: u8 = 1;
/// 方块消息里每个包能放的消息字节数（除去开头那一个标记字节）
const TILE_CHUNK: usize = RTP_PAYLOAD_MAX - 1;
/// 消息种类：这一帧哪些方块变了（时间戳 u32 + 位图）
const MSG_CHANGED: u8 = 1;
/// 消息种类：一个方块的无损数据（时间戳 u32 + 方块号 u16 + 数据）
const MSG_TILE: u8 = 2;
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
/// 被控端留多少个最近发过的包备补发：要覆盖主控端放弃等待（40 毫秒）之前发出的所有包。
/// 30 Mbps 下 40 毫秒只有约 130 个包，但关键帧上千个包是一口气交给发包节奏的；静止无损补发每帧最多约 700 个包
/// 插在画面之间（400 Mbps），40 毫秒约 1700 个，再加关键帧——留 8192 个（约 10 MB）
const HISTORY: usize = 8192;
/// 心跳也是授时探测，所以比「判断对方还在」需要的密得多：授时只取最近 2 秒里最好的样本
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(100);
pub const PEER_TIMEOUT: Duration = Duration::from_secs(3);
/// 关键帧一下子几十个包，系统默认的 UDP 缓冲（Windows 上很小）收慢一点就溢出
const SOCKET_BUFFER: usize = 8 << 20;

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
    /// 要发的帧发不了（没有 NAL、或 NAL 的类型 / 长度接收端认不出）
    BadFrame(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "udp: {e}"),
            Error::Srtp(e) => write!(f, "srtp: {e}"),
            Error::Timeout(what) => write!(f, "timed out: {what}"),
            Error::PeerGone => write!(f, "the peer stopped sending for {PEER_TIMEOUT:?}"),
            Error::BadFrame(e) => write!(f, "bad frame: {e}"),
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
/// 比收包缓冲大的包 Windows 报 10040（WSAEMSGSIZE）、包已丢掉（Linux 是截断后照常返回）：合法的包没这么大，
/// 是网上别的东西发来的，同样当没收到——否则同网段谁发一个大包就能打断通道。
fn is_timeout(e: &io::Error) -> bool {
    const WSAEMSGSIZE: i32 = 10040;
    matches!(
        e.kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused
    ) || (cfg!(windows) && e.raw_os_error() == Some(WSAEMSGSIZE))
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
    let pt = if orig.header.payload_type == PT_TILE { PT_RTX_TILE } else { PT_RTX };
    let mut p = rtp_packet(pt, ssrc, seq, orig.header.timestamp, orig.header.marker, payload);
    if let Some(ext) = orig.header.get_extension(EXT_ID) {
        p.header.set_extension(EXT_ID, ext).map_err(|e| Error::Srtp(e.to_string()))?;
    }
    Ok(p)
}

/// 补发包还原成原来的画面包 / 方块消息包（按补发载荷类型）。
fn rtx_unwrap(rtx: Packet, keys: &Keys) -> Option<Packet> {
    if rtx.payload.len() < RTX_OSN {
        return None;
    }
    let mut header = rtx.header;
    header.payload_type = if header.payload_type == PT_RTX_TILE { PT_TILE } else { PT_VIDEO };
    header.ssrc = keys.ssrc_media;
    header.sequence_number = u16::from_be_bytes([rtx.payload[0], rtx.payload[1]]);
    Some(Packet {
        header,
        payload: rtx.payload.slice(RTX_OSN..),
    })
}

mod receiver;
mod sender;

pub use receiver::{Event, Receiver, RecvStats};
pub use sender::{Listener, SendStats, Sender, Stamp};
