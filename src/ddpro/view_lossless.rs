//! 主控端静止无损（P1 第 7 项第 2 步之四）：被控端补来的无损方块在后台线程解出来，主线程放进着色器的无损纹理、
//! 在方块掩码里置上；画面说哪块变了就把那块撤掉。撤和画在同一次绘制里，不会出现旧图盖新画面。
//!
//! 每块记着「最近一次变化的帧的时间戳」；方块的像素取自时间戳为 ts 的那一帧，只有这块此后没再变过（ts 不早于它）才用。
//! 收包线程先交出帧、后交出方块，主线程每轮先处理完已到的帧再放方块，所以判断时该看到的变化都已经记上了。
//! 关键帧时被控端说整屏都变了（丢包、解码出错、本机重建着色器后都会要关键帧），掩码随之清空、被控端重新补。

use ddpro::{
    dec::GlConverter,
    media::tile::{self, Grid, TILE},
};
use hbb_common::{log, throttled_log};
use std::{
    io::Write,
    os::unix::net::UnixStream,
    sync::mpsc,
    time::Duration,
};

const WORKERS: usize = 3;
const LOG_INTERVAL: Duration = Duration::from_secs(5);

struct Job {
    ts: u32,
    index: u16,
    data: Vec<u8>,
}

struct Decoded {
    ts: u32,
    index: u16,
    result: Result<(u32, u32, Vec<u8>), String>,
}

/// 收包线程这一侧：把方块交给解压线程
pub(super) struct Decoders {
    jobs: Vec<mpsc::Sender<Job>>,
    next: usize,
}

impl Decoders {
    pub(super) fn submit(&mut self, ts: u32, index: u16, data: Vec<u8>) {
        // 解压线程只在本进程退出时才没了
        self.jobs[self.next].send(Job { ts, index, data }).ok();
        self.next = (self.next + 1) % self.jobs.len();
    }
}

/// 一段时间里的计数（日志用）
#[derive(Default, Debug)]
pub(super) struct Counts {
    /// 放进画面的方块
    pub applied: u64,
    /// 到的时候这块已经又变了，扔掉
    pub stale: u64,
    /// 解不出来或核对不上的（应当一直是 0）
    pub bad: u64,
    /// 当时不是 1:1 着色器显示，没法用
    pub unused: u64,
}

pub(super) struct Lossless {
    results: mpsc::Receiver<Decoded>,
    grid: Option<Grid>,
    changed_at: Vec<u32>,
    /// 掩码：每块一个字节，255 = 画面里这块是无损像素
    mask: Vec<u8>,
    mask_dirty: bool,
    /// 最近一次上屏的是整屏无损的画面（着色器画的、掩码是最新的）
    shown_full: bool,
    pub counts: Counts,
}

/// 开解压线程：解出一块就往 `wake` 写一个字节叫醒主线程。
pub(super) fn start(wake: &UnixStream) -> std::io::Result<(Decoders, Lossless)> {
    let (done_tx, results) = mpsc::channel();
    let mut jobs = Vec::with_capacity(WORKERS);
    for k in 0..WORKERS {
        let (tx, rx) = mpsc::channel::<Job>();
        let done_tx = done_tx.clone();
        let wake = wake.try_clone()?;
        std::thread::Builder::new()
            .name(format!("ddpro-view-tile-{k}"))
            .spawn(move || {
                for j in rx {
                    let result = tile::decode(&j.data).map_err(|e| e.to_string());
                    if done_tx
                        .send(Decoded {
                            ts: j.ts,
                            index: j.index,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    (&wake).write(&[1]).ok();
                }
            })?;
        jobs.push(tx);
    }
    Ok((
        Decoders { jobs, next: 0 },
        Lossless {
            results,
            grid: None,
            changed_at: Vec::new(),
            mask: Vec::new(),
            mask_dirty: true,
            shown_full: false,
            counts: Counts::default(),
        },
    ))
}

impl Lossless {
    /// 每收到一帧（不论解不解得出、显不显示）调：`changed` 是这一帧哪些方块变了。
    pub(super) fn changed(&mut self, ts: u32, changed: Option<&[u8]>) {
        let Some(bitmap) = changed else {
            return;
        };
        for t in 0..self.mask.len().min(bitmap.len() * 8) {
            if bitmap[t / 8] & (1 << (t % 8)) != 0 {
                self.changed_at[t] = ts;
                if self.mask[t] != 0 {
                    self.mask[t] = 0;
                    self.mask_dirty = true;
                }
            }
        }
    }

    /// 每解出一帧调：画面尺寸变了（被控端按新尺寸重建了编码器）就按新尺寸重来，当作整屏都在这一帧变了。
    pub(super) fn resize(&mut self, width: u32, height: u32, ts: u32) {
        if self.grid.map(|g| (g.width, g.height)) == Some((width, height)) {
            return;
        }
        let g = Grid::new(width, height);
        self.changed_at = vec![ts; g.count()];
        self.mask = vec![0; g.count()];
        self.mask_dirty = true;
        self.grid = Some(g);
    }

    /// 放进已解出的方块。`gl` 为 None（缩放显示、着色器不可用）时扔掉。
    pub(super) fn apply(&mut self, mut gl: Option<&mut GlConverter>) {
        while let Ok(d) = self.results.try_recv() {
            let (w, h, rgba) = match d.result {
                Ok(v) => v,
                Err(e) => {
                    self.counts.bad += 1;
                    throttled_log!(LOG_INTERVAL, warn, "ddpro: pro view lossless tile {} unusable: {e}", d.index);
                    continue;
                }
            };
            let (Some(g), Some(gl)) = (self.grid, gl.as_deref_mut()) else {
                self.counts.unused += 1;
                continue;
            };
            let t = d.index as usize;
            if t >= g.count() || (g.rect(t).2, g.rect(t).3) != (w, h) {
                self.counts.bad += 1;
                throttled_log!(LOG_INTERVAL, warn, "ddpro: pro view lossless tile {t} ({w}x{h}) does not fit a {}x{} frame", g.width, g.height);
                continue;
            }
            // 时间戳会回绕，按差值的正负比先后
            if (d.ts.wrapping_sub(self.changed_at[t]) as i32) < 0 {
                self.counts.stale += 1;
                continue;
            }
            let (x, y, ..) = g.rect(t);
            if let Err(e) = gl.put_tile(x, y, w, h, &rgba) {
                self.counts.bad += 1;
                throttled_log!(LOG_INTERVAL, warn, "ddpro: pro view cannot upload a lossless tile: {e}");
                continue;
            }
            self.counts.applied += 1;
            self.mask[t] = 255;
            self.mask_dirty = true;
        }
    }

    /// 画之前调：掩码变了就交给着色器。
    pub(super) fn update_mask(&mut self, gl: &mut GlConverter) {
        let Some(g) = self.grid else {
            return;
        };
        if !self.mask_dirty {
            return;
        }
        match gl.set_mask(TILE, g.cols, g.rows, &self.mask) {
            Ok(()) => self.mask_dirty = false,
            Err(e) => throttled_log!(LOG_INTERVAL, warn, "ddpro: pro view cannot update the lossless mask: {e}"),
        }
    }

    /// 无损纹理没了（本机重建了着色器转换器）：全部撤掉，调用方要关键帧让被控端重新补。
    pub(super) fn reset(&mut self) {
        self.mask.fill(0);
        self.mask_dirty = true;
        log::info!("ddpro: pro view lossless tiles cleared");
    }

    /// 上屏了一帧之后调：`by_shader` 是这一帧是不是着色器画的（缩放显示走视频处理单元，不算）。
    pub(super) fn presented(&mut self, by_shader: bool) {
        self.shown_full = by_shader && !self.mask_dirty && self.coverage() == Some(100);
    }

    /// 屏幕上已经是整屏无损的画面，此后没有任何方块变过、也没有新方块：再画一遍结果一字不差，可以不画。
    pub(super) fn settled(&self) -> bool {
        self.shown_full && !self.mask_dirty
    }

    /// 画面里无损方块占的比例（0–100），还没有画面时 None
    pub(super) fn coverage(&self) -> Option<u32> {
        let n = self.mask.len();
        (n > 0).then(|| (self.mask.iter().filter(|&&m| m != 0).count() * 100 / n) as u32)
    }

    /// 解出的画面尺寸（判断是否 1:1 显示）
    pub(super) fn size(&self) -> Option<(u32, u32)> {
        self.grid.map(|g| (g.width, g.height))
    }
}
