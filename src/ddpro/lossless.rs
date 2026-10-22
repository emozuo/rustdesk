//! 被控端静止无损的补发调度（P1 第 7 项第 2 步）：编码器每帧报出「哪些方块变了」，这里决定哪些方块该补、
//! 从显卡读回原始像素、交给后台线程压缩、按通道的方块预算发出去。全在画面线程里调，画面线程不等压缩也不等网络。
//!
//! 每块的状态：变了 → 待补（静止满 [`STILL_FRAMES`] 帧才读回）→ 读回后在压缩 / 等发送 → 已发出。
//! 读回时记下这块的「代数」，压完或要发时代数变了（这块又动过）就扔掉，不发旧图。主控端还按时间戳再挡一道。
//! 关键帧时整屏重来：主控端丢包要关键帧时清掉了全部无损方块，这里把这一帧当作全变了。
//! 补发速率有带宽保护（[`Rate`]）：链路扛不住就降，平稳了再加。

use ddpro::{
    enc::Encoder,
    media::{
        tile::{self, Grid},
        Sender,
    },
};
use hbb_common::{anyhow::anyhow, log, ResultType};
use std::{
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// 一块连续这么多帧没变才补（60 帧下 0.1 秒）：还在动的不白读白压
const STILL_FRAMES: u8 = 6;
/// 每帧最多读回多少块：整屏一次读回让那一帧晚出约 5 毫秒（P1 实施记录第 7 节），2560×1440 的 240 块分两帧读完
/// （读回的这两帧画面刚停下，晚出两三毫秒看不出来；原来 40 块要 6 帧，停下到补完约 270 毫秒）
const READS_PER_FRAME: usize = 120;
/// 读回了还没发出的（在压缩、压好待发）最多这么多块：压缩或网络跟不上时不再读，免得读一堆过时的
const MAX_IN_FLIGHT: usize = 3 * READS_PER_FRAME;
const WORKERS: usize = 3;
/// 零星几块的「补完」日志最多这么久一行（画面里有光标闪烁时几乎每 0.5 秒补完一次）
const LOG_INTERVAL: Duration = Duration::from_secs(5);
/// 整屏级的「补完」日志最多这么久一行（幻灯片、低帧率动画会一直整屏变）
const BIG_LOG_INTERVAL: Duration = Duration::from_secs(1);

// 带宽保护（P1 实施记录「带宽保护」）：补发速率按内网定的上限起步，链路扛不住（主控端大量要补发、或救不回来要了关键帧）就减半，
// 平稳一阵再慢慢加回去。思路同 RustDesk 的画面质量自适应（`src/server/video_qos.rs`：拥塞就降、恢复慢慢来、刚恢复的档位很快又拥塞
// 就压低），信号换成这条 UDP 通道上的丢包（RustDesk 那条是可靠连接，看的是延迟）
/// 补发速率最低降到多少（千比特每秒）
const RATE_MIN_KBPS: u32 = 10_000;
/// 多久看一次有没有拥塞
const ADAPT_WINDOW: Duration = Duration::from_millis(500);
/// 这段时间里主控端要求补发的包占发出的包超过这个比例算拥塞（WebRTC 拥塞控制按丢包降速的门槛也是 2%）
const LOSS_THRESHOLD: f64 = 0.02;
/// 补发请求少于这么多个不算（偶发的一两个丢包）
const LOSS_MIN_PACKETS: u64 = 10;
/// 平稳多久往上加一档、每档加多少
const RAISE_AFTER: Duration = Duration::from_secs(2);
const RAISE_FACTOR: f64 = 1.25;
/// 加上去之后这么快又拥塞，下次要等的时间翻倍，最多等这么久
const RAISE_AFTER_MAX: Duration = Duration::from_secs(16);
/// 补发期间救不回来、要了关键帧：补发先停这么久
const PAUSE_ON_KEYFRAME: Duration = Duration::from_secs(1);
/// 速率变化的日志最多这么久一行
const RATE_LOG_INTERVAL: Duration = Duration::from_secs(1);

/// 补发速率的带宽保护
struct Rate {
    max_kbps: u32,
    fps: u32,
    kbps: u32,
    window_start: Instant,
    /// 窗口开始时通道发出的包数、补发的包数
    base: (u64, u64),
    /// 上次降速 / 加速之后多久没拥塞了
    calm_since: Instant,
    raise_after: Duration,
    raised_at: Option<Instant>,
    paused_until: Option<Instant>,
    /// 最近一次发出方块的时刻：只有在补发的时候拥塞才算补发的账，没在补发时也不往上加（没有凭据）
    last_tile: Option<Instant>,
    last_log: Option<Instant>,
}

impl Rate {
    fn new(max_kbps: u32, fps: u32) -> Self {
        let now = Instant::now();
        Self {
            max_kbps,
            fps,
            kbps: max_kbps,
            window_start: now,
            base: (0, 0),
            calm_since: now,
            raise_after: RAISE_AFTER,
            raised_at: None,
            paused_until: None,
            last_tile: None,
            last_log: None,
        }
    }

    fn sending_lately(&self, now: Instant) -> bool {
        self.last_tile.is_some_and(|t| now.duration_since(t) < ADAPT_WINDOW * 2)
    }

    /// 每帧发完之后调：`keyframe` 是这一帧是不是应主控端要求出的关键帧（救不回来的丢包、解码出错等）
    fn adapt(&mut self, sender: &mut Sender, keyframe: bool) {
        let now = Instant::now();
        if keyframe && self.sending_lately(now) {
            self.paused_until = Some(now + PAUSE_ON_KEYFRAME);
            self.lower(sender, now, "the controller asked for a keyframe while tiles were being sent");
        }
        if now.duration_since(self.window_start) < ADAPT_WINDOW {
            return;
        }
        let (packets, rtx) = (sender.stats.packets, sender.stats.retransmitted);
        let (sent, lost) = (packets.saturating_sub(self.base.0), rtx.saturating_sub(self.base.1));
        self.base = (packets, rtx);
        self.window_start = now;
        if lost >= LOSS_MIN_PACKETS && lost as f64 > LOSS_THRESHOLD * sent as f64 {
            if self.sending_lately(now) {
                self.lower(sender, now, &format!("{lost} of {sent} packets resent in {} ms", ADAPT_WINDOW.as_millis()));
            }
        } else if self.kbps < self.max_kbps
            && self.sending_lately(now)
            && now.duration_since(self.calm_since) >= self.raise_after
        {
            self.kbps = ((self.kbps as f64 * RAISE_FACTOR) as u32).min(self.max_kbps);
            self.raised_at = Some(now);
            self.calm_since = now;
            if self.kbps == self.max_kbps {
                self.raise_after = RAISE_AFTER;
            }
            self.apply(sender, now, "no congestion");
        }
    }

    fn lower(&mut self, sender: &mut Sender, now: Instant, why: &str) {
        // 刚加上去就拥塞：这一档不稳，下次多等一阵再加
        if self.raised_at.is_some_and(|t| now.duration_since(t) < self.raise_after) {
            self.raise_after = (self.raise_after * 2).min(RAISE_AFTER_MAX);
        }
        self.raised_at = None;
        self.calm_since = now;
        if self.kbps > RATE_MIN_KBPS {
            self.kbps = (self.kbps / 2).max(RATE_MIN_KBPS);
            self.apply(sender, now, why);
        }
    }

    fn apply(&mut self, sender: &mut Sender, now: Instant, why: &str) {
        sender.pace_tiles(self.kbps, self.fps);
        if self.last_log.map_or(true, |t| now.duration_since(t) >= RATE_LOG_INTERVAL) {
            self.last_log = Some(now);
            log::info!("ddpro: lossless rate {} Mbps ({why})", self.kbps / 1000);
        }
    }

    fn paused(&self) -> bool {
        self.paused_until.is_some_and(|t| Instant::now() < t)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// 当前内容还没补
    Dirty,
    /// 读回了，在压缩或等发送
    Pending,
    Sent,
}

struct Job {
    index: usize,
    generation: u32,
    ts: u32,
    w: u32,
    h: u32,
    bgra: Vec<u8>,
}

struct Done {
    index: usize,
    generation: u32,
    ts: u32,
    data: Vec<u8>,
}

pub(super) struct Lossless {
    grid: Grid,
    state: Vec<State>,
    /// 每块连续多少帧没变（到 [`STILL_FRAMES`] 封顶）
    still: Vec<u8>,
    generation: Vec<u32>,
    jobs: Vec<mpsc::Sender<Job>>,
    next_worker: usize,
    done: mpsc::Receiver<ResultType<Done>>,
    workers: Vec<JoinHandle<()>>,
    /// 压好了、预算不够留到下一帧发的
    ready: Vec<Done>,
    in_flight: usize,
    /// 最近一次有方块变化的时刻，和那之后补发的量（日志用）
    last_change: Instant,
    since_change: (u64, u64),
    complete: bool,
    last_log: Option<Instant>,
    rate: Rate,
}

impl Lossless {
    /// `max_kbps` / `fps`：补发速率的上限（通道上已按它设好方块预算）和画面帧率，带宽保护往下调时照这两个重设预算
    pub(super) fn new(width: u32, height: u32, max_kbps: u32, fps: u32) -> ResultType<Self> {
        let grid = Grid::new(width, height);
        let n = grid.count();
        if n > u16::MAX as usize + 1 {
            return Err(anyhow!("{width}x{height} has too many tiles"));
        }
        let (done_tx, done) = mpsc::channel();
        let mut jobs = Vec::with_capacity(WORKERS);
        let mut workers = Vec::with_capacity(WORKERS);
        for k in 0..WORKERS {
            let (tx, rx) = mpsc::channel::<Job>();
            let done_tx = done_tx.clone();
            workers.push(
                std::thread::Builder::new()
                    .name(format!("ddpro-tile-{k}"))
                    .spawn(move || {
                        for j in rx {
                            let mut data = Vec::new();
                            let r = tile::encode(&j.bgra, j.w, j.h, &mut data)
                                .map(|_| Done {
                                    index: j.index,
                                    generation: j.generation,
                                    ts: j.ts,
                                    data,
                                })
                                .map_err(|e| anyhow!("tile {}: {e}", j.index));
                            if done_tx.send(r).is_err() {
                                break;
                            }
                        }
                    })?,
            );
            jobs.push(tx);
        }
        Ok(Self {
            grid,
            state: vec![State::Dirty; n],
            still: vec![0; n],
            generation: vec![0; n],
            jobs,
            next_worker: 0,
            done,
            workers,
            ready: Vec::new(),
            in_flight: 0,
            last_change: Instant::now(),
            since_change: (0, 0),
            complete: false,
            last_log: None,
            rate: Rate::new(max_kbps, fps),
        })
    }

    /// 这一帧发出之前调：记下哪些方块变了，返回要发给主控端的位图（没有方块变就是 `None`，不用发）。
    pub(super) fn changed(&mut self, bitmap: Option<Vec<u8>>, keyframe: bool) -> Option<Vec<u8>> {
        let n = self.grid.count();
        let bitmap = if keyframe {
            vec![0xff; n.div_ceil(8)]
        } else {
            bitmap?
        };
        let mut any = false;
        for t in 0..n {
            if bitmap[t / 8] & (1 << (t % 8)) != 0 {
                any = true;
                self.state[t] = State::Dirty;
                self.still[t] = 0;
                self.generation[t] = self.generation[t].wrapping_add(1);
            } else {
                self.still[t] = self.still[t].saturating_add(1).min(STILL_FRAMES);
            }
        }
        if !any {
            return None;
        }
        self.last_change = Instant::now();
        self.since_change = (0, 0);
        self.complete = false;
        Some(bitmap)
    }

    /// 最近一次变化之后的方块都补发出去了
    pub(super) fn complete(&self) -> bool {
        self.complete
    }

    /// 每个节拍调（这一拍编没编都调）：读回该补的方块交去压缩，压好的按预算发出去。`ts` 是这一拍的时间戳（读回的像素取自它），
    /// `keyframe`：这一拍编出的是关键帧（带宽保护看它）
    pub(super) fn pump(&mut self, enc: &mut Encoder, sender: &mut Sender, ts: u32, keyframe: bool) -> ResultType<()> {
        self.rate.adapt(sender, keyframe);
        self.read(enc, ts)?;
        while let Ok(r) = self.done.try_recv() {
            let d = r?;
            self.in_flight -= 1;
            self.ready.push(d);
        }
        self.send(sender)?;
        if !self.complete && self.in_flight == 0 && self.ready.is_empty() && self.state.iter().all(|s| *s == State::Sent) {
            self.complete = true;
            // 补了四分之一屏以上的基本都记（量整屏补完的时间和数据量），零星几块的多节流一些
            let big = self.since_change.0 as usize * 4 >= self.grid.count();
            let interval = if big { BIG_LOG_INTERVAL } else { LOG_INTERVAL };
            if self.last_log.map_or(true, |t| t.elapsed() >= interval) {
                self.last_log = Some(Instant::now());
                log::info!(
                    "ddpro: lossless complete {} ms after the last change: {} tiles, {} bytes",
                    self.last_change.elapsed().as_millis(),
                    self.since_change.0,
                    self.since_change.1
                );
            }
        }
        Ok(())
    }

    fn read(&mut self, enc: &mut Encoder, ts: u32) -> ResultType<()> {
        // 压好待发的也算在内（发不完的会越攒越多）；里面已经过时的先扔掉，不占名额
        let generation = &self.generation;
        self.ready.retain(|d| d.generation == generation[d.index]);
        let room = MAX_IN_FLIGHT
            .saturating_sub(self.in_flight + self.ready.len())
            .min(READS_PER_FRAME);
        let picked: Vec<usize> = (0..self.grid.count())
            .filter(|&t| self.state[t] == State::Dirty && self.still[t] >= STILL_FRAMES)
            .take(room)
            .collect();
        if picked.is_empty() {
            return Ok(());
        }
        let rects: Vec<_> = picked.iter().map(|&t| self.grid.rect(t)).collect();
        let pixels = enc.read_tiles(&rects).map_err(|e| anyhow!("{e}"))?;
        for ((&index, (_, _, w, h)), bgra) in picked.iter().zip(rects).zip(pixels) {
            self.state[index] = State::Pending;
            let job = Job {
                index,
                generation: self.generation[index],
                ts,
                w,
                h,
                bgra,
            };
            self.jobs[self.next_worker]
                .send(job)
                .map_err(|_| anyhow!("tile worker gone"))?;
            self.next_worker = (self.next_worker + 1) % WORKERS;
            self.in_flight += 1;
        }
        Ok(())
    }

    /// 按方块预算发：有余量就发（可能超出一块的量，记到下一帧的账上），过时的扔掉
    fn send(&mut self, sender: &mut Sender) -> ResultType<()> {
        if self.rate.paused() {
            return Ok(());
        }
        while let Some(d) = self.ready.pop() {
            if d.generation != self.generation[d.index] {
                continue;
            }
            if sender.tile_room() == 0 {
                self.ready.push(d);
                break;
            }
            sender
                .send_tile(d.ts, d.index as u16, &d.data)
                .map_err(|e| anyhow!("{e}"))?;
            self.state[d.index] = State::Sent;
            self.rate.last_tile = Some(Instant::now());
            self.since_change.0 += 1;
            self.since_change.1 += d.data.len() as u64;
        }
        Ok(())
    }
}

impl Drop for Lossless {
    fn drop(&mut self) {
        self.jobs.clear();
        for w in self.workers.drain(..) {
            w.join().ok();
        }
    }
}
