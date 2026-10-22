//! 被控端：开端口等主控端打招呼、发画面（发包节奏）、收心跳和补发请求（补发）。

use super::{
    control, is_timeout, micros, micros_u32, open, read_i64, rtp_packet, rtx_packet, seal, srtp, udp, Error, Keys,
    CTRL_BYE, CTRL_HELLO, CTRL_KEEPALIVE, CTRL_KEYFRAME, CTRL_NACK, CTRL_TIME_ECHO, HISTORY, MSG_CHANGED, MSG_TILE,
    PEER_TIMEOUT, PT_CONTROL, PT_TILE, PT_VIDEO, RTP_HEADER, RTP_PAYLOAD_MAX, SRTP_GCM_TAG, TILE_CHUNK, TILE_FIRST,
    UDP_PAYLOAD,
};
use crate::{
    hevc,
    timing::{SenderStamp, EXT_ID, EXT_OVERHEAD},
};
use bytes::Bytes;
use rand::Rng;
use rtp::packet::Packet;
use std::{
    io,
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use webrtc_srtp::context::Context;

/// 发包节奏：超出一帧预算的部分按平均码率的这么多倍匀速发（WebRTC 的默认倍数）
const PACE_FACTOR: f64 = 2.5;
/// 欠账超过这么多字节才睡：逐包睡的话，睡眠精度远粗于一个包的发送间隔
const PACE_SLACK: f64 = (4 * UDP_PAYLOAD) as f64;
/// 被控端收心跳的线程多久看一次「该停了」
const CONTROL_POLL: Duration = Duration::from_millis(100);

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
    /// `factor`：补充速度是平均码率的几倍——画面用 [`PACE_FACTOR`]；无损方块用 1（按给定码率匀速补充）
    fn new(kbps: u32, fps: u32, factor: f64) -> Self {
        let bytes_per_sec = kbps as f64 * 1000.0 / 8.0;
        let burst = bytes_per_sec / fps.max(1) as f64;
        Self {
            rate: bytes_per_sec * factor,
            burst,
            tokens: burst,
            last: Instant::now(),
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * self.rate).min(self.burst);
        self.last = now;
    }

    fn take(&mut self, len: usize) {
        self.refill();
        self.tokens -= len as f64;
        if self.tokens < -PACE_SLACK {
            std::thread::sleep(Duration::from_secs_f64(-self.tokens / self.rate));
        }
    }

    /// 现在不用等就能发的字节数
    fn room(&mut self) -> usize {
        self.refill();
        self.tokens.max(0.0) as usize
    }
}

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
        // 先收一次再看截止时间：重建后接着等时剩下的时间可能已经是 0，但主控端的招呼可能早就在缓冲里了
        loop {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    let hello = open(&mut dec, &buf[..n]).is_some_and(|p| matches!(control(&p, &self.keys), Some((CTRL_HELLO, _))));
                    if hello {
                        return Sender::start(self, from, dec);
                    }
                }
                Err(e) if is_timeout(&e) => {}
                Err(e) => return Err(e.into()),
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout("no hello from the controller on the pro channel"));
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
    /// 发出的无损方块和它们的字节数（不含「哪些方块变了」的消息）
    pub tiles: u64,
    pub tile_bytes: u64,
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
    /// 无损方块的预算（[`Sender::pace_tiles`]）；没设就不限
    tile_pacer: Option<Pacer>,
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
            tile_pacer: None,
            stats: SendStats::default(),
        })
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// 被控端时间轴的零点（同 [`Listener::epoch`]）
    pub fn epoch(&self) -> Instant {
        self.epoch
    }

    /// 打开发包节奏：`kbps` / `fps` 是编码器的平均码率和帧率；码率为 0 或不调就一口气发（自测用）。
    pub fn pace(&mut self, kbps: u32, fps: u32) {
        let on = kbps > 0;
        self.pacer = on.then(|| Pacer::new(kbps, fps, PACE_FACTOR));
        let packed = if on { (kbps as u64) << 32 | fps as u64 } else { 0 };
        self.rtx.pace.store(packed, Ordering::Relaxed);
    }

    /// 发一帧（带起始码的 H.265 码流）。`ts` 是 90 kHz 的 RTP 时间戳；`stamp` 有值时在首包里带上
    /// 这一帧的被控端时间（重复上一帧画面等不代表新画面的帧不带）。
    pub fn send_frame(&mut self, annexb: &[u8], ts: u32, stamp: Option<Stamp>) -> Result<(), Error> {
        let nals = hevc::split_annexb(annexb);
        if nals.is_empty() {
            return Err(Error::BadFrame("no NAL unit in the frame".into()));
        }
        // 接收端只认单个 NAL 和分片两种包：比 NAL 头还短的、类型落在聚合包 / 分片 / 保留区间（48–63）的，
        // 原样发过去会被当成别的包解析
        if let Some(nal) = nals.iter().find(|n| n.len() < hevc::NAL_HEADER || hevc::nal_type(n) >= 48) {
            return Err(Error::BadFrame(format!("unsendable NAL unit (type {}, {} bytes)", hevc::nal_type(nal), nal.len())));
        }
        let mut payloads = Vec::new();
        for (k, nal) in nals.into_iter().enumerate() {
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
            let stamped = i == 0 && stamp.is_some();
            // 带时间的首包先过发包节奏再打时间戳：被节奏挡住的等待算进「发出」一段，不落进跨机器的「网络」一段
            if stamped {
                self.wait_pace(RTP_HEADER + pkt.payload.len() + EXT_OVERHEAD + SRTP_GCM_TAG);
            }
            if let (true, Some(s)) = (stamped, stamp) {
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
            if !stamped {
                self.wait_pace(data.len());
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

    fn wait_pace(&mut self, len: usize) {
        if let Some(p) = self.pacer.as_mut() {
            p.take(len);
        }
    }

    /// 无损方块的预算：`kbps` 平均码率（内网实测 800 Mbps 突发不丢包，见 P1 实施记录第 7 节），`fps` 画面帧率
    /// （桶容量是一帧的量）。0 = 不限。
    pub fn pace_tiles(&mut self, kbps: u32, fps: u32) {
        self.tile_pacer = (kbps > 0).then(|| Pacer::new(kbps, fps, 1.0));
    }

    /// 现在不用等就能发多少字节的方块（按 [`Sender::pace_tiles`] 的预算）。调用方据此决定这一帧再发哪些方块，
    /// 发不下的留到下一帧——画面循环不会因为补发停下来等。
    pub fn tile_room(&mut self) -> usize {
        self.tile_pacer.as_mut().map_or(usize::MAX, Pacer::room)
    }

    /// 告诉主控端时间戳为 `ts` 的这一帧哪些方块变了（`bitmap` 每位一块、低位在前），紧接着要发的就是这一帧。
    /// 没有方块变化就不用发。
    pub fn send_changed(&mut self, ts: u32, bitmap: &[u8]) -> Result<(), Error> {
        let mut msg = Vec::with_capacity(5 + bitmap.len());
        msg.push(MSG_CHANGED);
        msg.extend_from_slice(&ts.to_le_bytes());
        msg.extend_from_slice(bitmap);
        self.send_message(&msg, ts, false)
    }

    /// 发一个无损方块（[`crate::tile::encode`] 编好的），`ts` 是它的像素取自的那一帧的时间戳，`index` 是方块号。
    /// 只在两帧之间调（不能插进一帧中间）；不等预算，调用方先看 [`Sender::tile_room`]。
    pub fn send_tile(&mut self, ts: u32, index: u16, data: &[u8]) -> Result<(), Error> {
        let mut msg = Vec::with_capacity(7 + data.len());
        msg.push(MSG_TILE);
        msg.extend_from_slice(&ts.to_le_bytes());
        msg.extend_from_slice(&index.to_le_bytes());
        msg.extend_from_slice(data);
        self.send_message(&msg, ts, true)?;
        self.stats.tiles += 1;
        self.stats.tile_bytes += data.len() as u64;
        Ok(())
    }

    /// 一条方块消息切成连续的包发出：首包开头标出「消息开头」，末包打结束标记。方块数据记在方块预算上（不等），
    /// 「哪些方块变了」很小，和画面一起走画面的发包节奏
    fn send_message(&mut self, msg: &[u8], ts: u32, tile: bool) -> Result<(), Error> {
        let chunks: Vec<&[u8]> = msg.chunks(TILE_CHUNK).collect();
        let last = chunks.len().saturating_sub(1);
        for (i, chunk) in chunks.into_iter().enumerate() {
            let mut payload = Vec::with_capacity(1 + chunk.len());
            payload.push(if i == 0 { TILE_FIRST } else { 0 });
            payload.extend_from_slice(chunk);
            let pkt = rtp_packet(PT_TILE, self.keys.ssrc_media, self.seq, ts, i == last, payload);
            self.seq = self.seq.wrapping_add(1);
            let data = seal(&mut self.enc, &pkt)?;
            self.history.lock().unwrap().put(pkt);
            match (tile, self.tile_pacer.as_mut()) {
                (true, Some(p)) => {
                    p.refill();
                    p.tokens -= data.len() as f64;
                }
                (true, None) => {}
                (false, _) => self.wait_pace(data.len()),
            }
            self.sock.send_to(&data, self.peer)?;
            self.stats.packets += 1;
            self.stats.bytes += data.len() as u64;
        }
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
            self.pacer = (pace != 0).then(|| (pace, Pacer::new((pace >> 32) as u32, pace as u32, PACE_FACTOR)));
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
