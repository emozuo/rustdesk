//! 主控端：打招呼、发心跳（兼授时）、收画面（按序号重排、请求补发、救不回来才请求关键帧）。

use super::{
    is_timeout, micros, open, read_i64, rtp_packet, rtx_unwrap, seal, srtp, udp, unspecified_like, Error, Keys,
    CTRL_BYE, CTRL_HELLO, CTRL_KEEPALIVE, CTRL_KEYFRAME, CTRL_NACK, CTRL_TIME_ECHO, HISTORY, KEEPALIVE_INTERVAL, MSG_CHANGED,
    MSG_TILE, PEER_TIMEOUT, PT_CONTROL, PT_RTX, PT_RTX_TILE, PT_TILE, PT_VIDEO, TILE_FIRST,
};
use crate::{
    hevc::{self, DepacketizeError, Depacketizer},
    timing::{ClockEstimate, ClockSync, FrameTimes, SenderStamp, EXT_ID},
};
use rand::Rng;
use rtp::packet::Packet;
use std::{
    collections::{HashMap, VecDeque},
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};
use webrtc_srtp::context::Context;

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
/// 等关键帧期间，隔这么久还没等到就再要一次（上一次请求或关键帧本身可能也丢了）
const LOSS_REPEAT: Duration = Duration::from_millis(200);
/// 等关键帧期间又出现救不回来的丢包（多半是被控端应请求发来的关键帧也丢了片）：最快隔这么久再要一次，
/// 免得一个关键帧的几十个分片各自触发一次请求
const LOSS_REREQUEST: Duration = Duration::from_millis(100);
/// 一个补发请求里最多列多少个序号（控制包不超过 UDP_PAYLOAD）
const NACK_PER_PACKET: usize = 256;
/// 一条方块消息最长多少字节（一块 128×128 的数据最多约 49 KB，「哪些方块变了」的位图更小）
const MSG_MAX: usize = 256 * 1024;

pub enum Event {
    /// 收齐的一帧（带起始码的 H.265 码流）。丢包之后，第一个收到的总是关键帧。
    /// `times` 是这一帧的分段时间，被控端没带时间（如重复上一帧画面）时为 None。
    /// `changed` 是被控端说这一帧哪些方块变了（每位一块、低位在前）；None 表示没有方块变化。
    Frame {
        data: Vec<u8>,
        timestamp: u32,
        times: Option<FrameTimes>,
        changed: Option<Vec<u8>>,
    },
    /// 一个无损方块（[`crate::tile::decode`] 解）：`timestamp` 是它的像素取自的那一帧的时间戳，`index` 是方块号。
    /// 等关键帧期间（丢包之后）收到的不交出去——随后的关键帧带「整屏都变了」，调用方据此撤掉所有方块，被控端重新补。
    Tile { timestamp: u32, index: u16, data: Vec<u8> },
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
    /// 解码出错后要求重新同步（等关键帧）的次数
    pub resyncs: u64,
    /// 丢包后到关键帧之前收到、因为解不了而丢掉的帧
    pub discarded_frames: u64,
    /// 解密 / 认证失败的包（不是本会话的或被篡改的）
    pub auth_failures: u64,
    /// 诊断开关故意丢掉的包
    pub diag_dropped: u64,
    /// 交出去的无损方块和它们的字节数
    pub tiles: u64,
    pub tile_bytes: u64,
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
    /// 正在收的这一帧（上一个帧结束标记之后）没出过救不回来的丢包
    frame_clean: bool,
    last_loss_event: Option<Instant>,
    pending: VecDeque<Event>,
    drop_permille: u32,
    jitter: Option<(f64, f64)>,
    start: Instant,
    clock: ClockSync,
    /// 正在收的这一帧：首包到达时刻、被控端带来的时间
    frame_first: Option<i64>,
    frame_stamp: Option<SenderStamp>,
    /// 正在拼的方块消息（收到「消息开头」之后）
    msg: Option<Vec<u8>>,
    /// 「哪些方块变了」：等时间戳相同的那一帧收齐时挂上去
    changed: Option<(u32, Vec<u8>)>,
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
            frame_clean: true,
            last_loss_event: Some(now),
            pending: VecDeque::new(),
            drop_permille,
            jitter: None,
            start: now,
            clock: ClockSync::default(),
            frame_first: None,
            frame_stamp: None,
            msg: None,
            changed: None,
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

    /// 告诉被控端专业画面不要了（关专业窗口时调），它立即退回通用画面。连发三次：控制包丢了也不至于让它
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

    /// 解码那边出了错（不是丢包，比如码流坏了或驱动报错）：参考帧已经不可信，照救不回来的丢包处理——
    /// 丢掉正在拼的帧、之后的帧在关键帧到之前都不交出去，并在通道上请求关键帧（已经在等关键帧时按原来的
    /// 间隔再要，不会每帧都要一次）。
    pub fn resync(&mut self) {
        self.stats.resyncs += 1;
        self.loss(0);
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
            (PT_VIDEO | PT_TILE, ssrc) if ssrc == self.keys.ssrc_media => p,
            // 补发只对还在缺包表里的有用（末尾试探也先把序号放进表里）：已经放弃、或原包先到了的晚到补发扔掉。整屏补发时
            // 序号走得快，晚到约 0.8 秒的补发包序号会被当成很远的将来，误判成大断档或占住将来那个包的位置
            (PT_RTX | PT_RTX_TILE, ssrc) if ssrc == self.keys.ssrc_rtx() => match rtx_unwrap(p, &self.keys) {
                Some(p) if self.missing.contains_key(&p.header.sequence_number) => p,
                _ => return Ok(()),
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
            let stalled = (self.frame_first.is_some() || self.msg.is_some()) && self.last_media.elapsed() >= TAIL_PROBE;
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
        if p.header.payload_type == PT_TILE {
            self.feed_message(p);
            return;
        }
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
        // 这一帧收的过程中出过救不回来的丢包：后面的包哪怕拼得起来，缺的可能正是开头的参数集
        // （拼出一个缺参数集的「关键帧」交给解码器），整帧不要。帧结束，下一帧重新算
        let clean = std::mem::replace(&mut self.frame_clean, true);
        let frame = match self.depack.take_frame() {
            Ok(f) if clean => f,
            Ok(_) => {
                self.stats.discarded_frames += 1;
                return;
            }
            Err(_) => {
                self.loss(0);
                self.frame_clean = true;
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
        // 「哪些方块变了」紧贴在这一帧前面；时间戳对不上的（前面的帧被丢掉了）作废
        let changed = self
            .changed
            .take()
            .filter(|(ts, _)| *ts == p.header.timestamp)
            .map(|(_, bitmap)| bitmap);
        self.pending.push_back(Event::Frame {
            data: frame,
            timestamp: p.header.timestamp,
            times: stamp.map(|s| FrameTimes::new(s, offset, first, done)),
            changed,
        });
    }

    /// 方块消息的包：按「消息开头」标记和结束标记拼成整条消息。中途丢了包的残消息在 [`Receiver::loss`] 里扔掉；
    /// 丢包后从中间开始收到的（没见到开头）跳过，直到下一条消息开头。
    fn feed_message(&mut self, p: Packet) {
        let Some((&flags, body)) = p.payload.split_first() else {
            return;
        };
        if flags & TILE_FIRST != 0 {
            self.msg = Some(Vec::new());
        }
        let Some(msg) = self.msg.as_mut() else {
            return;
        };
        msg.extend_from_slice(body);
        // 合法消息远小于此（一块最多约 49 KB）：一直不来结束标记的不再往上攒
        if msg.len() > MSG_MAX {
            self.msg = None;
            return;
        }
        if !p.header.marker {
            return;
        }
        let Some(msg) = self.msg.take() else {
            return;
        };
        if msg.len() < 5 {
            return;
        }
        let ts = u32::from_le_bytes([msg[1], msg[2], msg[3], msg[4]]);
        match msg[0] {
            MSG_CHANGED => self.changed = Some((ts, msg[5..].to_vec())),
            MSG_TILE if msg.len() >= 7 && !self.waiting_irap => {
                let index = u16::from_le_bytes([msg[5], msg[6]]);
                let data = msg[7..].to_vec();
                self.stats.tiles += 1;
                self.stats.tile_bytes += data.len() as u64;
                self.pending.push_back(Event::Tile { timestamp: ts, index, data });
            }
            _ => {}
        }
    }

    /// 发现丢包（`packets` 是序号间隙；0 表示分片残缺等由丢包引起的后果）。
    fn loss(&mut self, packets: u32) {
        self.stats.lost_packets += packets as u64;
        self.depack.reset();
        self.keyframe_arriving = false;
        self.frame_clean = false;
        // 残帧的首包时间和被控端时间都作废，不能带到下一帧；拼了一半的方块消息、没挂上去的「哪些方块变了」也作废
        // （调用方不用为丢包撤方块：被控端随后的关键帧带「整屏都变了」，挂在关键帧上撤）
        self.frame_first = None;
        self.frame_stamp = None;
        self.msg = None;
        self.changed = None;
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
