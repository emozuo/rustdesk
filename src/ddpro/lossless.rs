//! 被控端静止无损的补发调度（P1 第 7 项第 2 步）：编码器每帧报出「哪些方块变了」，这里决定哪些方块该补、
//! 从显卡读回原始像素、交给后台线程压缩、按通道的方块预算发出去。全在画面线程里调，画面线程不等压缩也不等网络。
//!
//! 每块的状态：变了 → 待补（静止满 [`STILL_FRAMES`] 帧才读回）→ 读回后在压缩 / 等发送 → 已发出。
//! 读回时记下这块的「代数」，压完或要发时代数变了（这块又动过）就扔掉，不发旧图。主控端还按时间戳再挡一道。
//! 关键帧时整屏重来：主控端丢包要关键帧时清掉了全部无损方块，这里把这一帧当作全变了。

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
/// 每帧最多读回多少块：整屏一次读回让那一帧晚出约 5 毫秒（P1 实施记录第 7 节），分散到几帧里
const READS_PER_FRAME: usize = 40;
/// 读回了还没发出的最多这么多块：压缩或网络跟不上时不再读，免得读一堆过时的
const MAX_IN_FLIGHT: usize = 3 * READS_PER_FRAME;
const WORKERS: usize = 3;
/// 零星几块的「补完」日志最多这么久一行（画面里有光标闪烁时几乎每 0.5 秒补完一次）
const LOG_INTERVAL: Duration = Duration::from_secs(5);

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
}

impl Lossless {
    pub(super) fn new(width: u32, height: u32) -> ResultType<Self> {
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

    /// 这一帧发出之后调：读回该补的方块交去压缩，压好的按预算发出去。`ts` 是刚发的这一帧的时间戳（读回的像素取自它）。
    pub(super) fn pump(&mut self, enc: &mut Encoder, sender: &mut Sender, ts: u32) -> ResultType<()> {
        self.read(enc, ts)?;
        while let Ok(r) = self.done.try_recv() {
            let d = r?;
            self.in_flight -= 1;
            self.ready.push(d);
        }
        self.send(sender)?;
        if !self.complete && self.in_flight == 0 && self.ready.is_empty() && self.state.iter().all(|s| *s == State::Sent) {
            self.complete = true;
            // 补了四分之一屏以上的每次都记（量整屏补完的时间和数据量），零星几块的节流
            let big = self.since_change.0 as usize * 4 >= self.grid.count();
            if big || self.last_log.map_or(true, |t| t.elapsed() >= LOG_INTERVAL) {
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
        let room = MAX_IN_FLIGHT.saturating_sub(self.in_flight).min(READS_PER_FRAME);
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
