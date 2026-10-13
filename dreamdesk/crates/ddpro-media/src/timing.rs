//! 端到端分段延迟：两端对时钟、每帧随首包带过来的被控端时间、按段记账。
//!
//! 时间一律是微秒，各自以本端通道建立时为零点（[`crate::Listener::epoch`] / [`crate::Receiver::epoch`]）。
//! 只有「网络」一段跨机器，靠 [`ClockSync`] 把被控端时间换算到主控端时间轴上；其余各段都是同一台机器上的差值。

use std::{collections::VecDeque, fmt};

/// 每帧首包 RTP 包头扩展（RFC 8285 单字节头）里放被控端时间的那一项。
pub(crate) const EXT_ID: u8 = 1;
pub(crate) const EXT_LEN: usize = 16;
/// 带这一项时包头多出的字节：扩展头 4 + (项头 1 + 数据 16，补齐到 4 的倍数) 20
pub(crate) const EXT_OVERHEAD: usize = 4 + (1 + EXT_LEN).div_ceil(4) * 4;

/// 取最近这么长时间里往返最快的那次授时。两台机器的时钟每秒会相对走偏几十微秒，
/// 窗口太长用到的是旧的时钟差；太短又可能挑不到没排队的样本。
const SYNC_WINDOW_US: i64 = 2_000_000;
const SYNC_SAMPLES: usize = 64;
/// 窗口里至少这么多次授时才给结果。只有一两次时挑不出没排队的那次：209.32 实测通道刚打开的头 0.1 秒
/// 时钟差偏了 1.5 ms（网络一段成了负数），之后稳定。
const SYNC_MIN_SAMPLES: usize = 5;

/// 被控端随帧带过来的时间（被控端时钟）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SenderStamp {
    /// 拿到画面的时刻
    pub captured_us: u64,
    /// 拿到画面 → 编码完成
    pub encode_us: u32,
    /// 编码完成 → 首包发出
    pub queue_us: u32,
}

impl SenderStamp {
    pub fn to_bytes(self) -> [u8; EXT_LEN] {
        let mut b = [0u8; EXT_LEN];
        b[..8].copy_from_slice(&self.captured_us.to_le_bytes());
        b[8..12].copy_from_slice(&self.encode_us.to_le_bytes());
        b[12..].copy_from_slice(&self.queue_us.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let b: &[u8; EXT_LEN] = b.try_into().ok()?;
        Some(Self {
            captured_us: u64::from_le_bytes(b[..8].try_into().ok()?),
            encode_us: u32::from_le_bytes(b[8..12].try_into().ok()?),
            queue_us: u32::from_le_bytes(b[12..].try_into().ok()?),
        })
    }

    fn sent_us(&self) -> i64 {
        self.captured_us as i64 + self.encode_us as i64 + self.queue_us as i64
    }
}

/// 一帧在主控端收齐时的时间账（主控端时钟）。
#[derive(Debug, Clone, Copy)]
pub struct FrameTimes {
    pub encode_us: u32,
    pub queue_us: u32,
    /// 首包发出的时刻换算到本端时钟；还没对上时钟时为 None
    pub sent_at: Option<i64>,
    pub first_arrival: i64,
    pub last_arrival: i64,
}

impl FrameTimes {
    pub(crate) fn new(stamp: SenderStamp, offset: Option<i64>, first_arrival: i64, last_arrival: i64) -> Self {
        Self {
            encode_us: stamp.encode_us,
            queue_us: stamp.queue_us,
            sent_at: offset.map(|o| stamp.sent_us() - o),
            first_arrival,
            last_arrival,
        }
    }

    /// 被控端拿到画面的时刻（本端时钟）。
    pub fn captured_at(&self) -> Option<i64> {
        self.sent_at.map(|s| s - self.queue_us as i64 - self.encode_us as i64)
    }

    /// 编码、发出、网络、到齐四段；时钟还没对上（网络一段算不出）时返回 None。
    pub fn transport_stages(&self) -> Option<[(Stage, i64); 4]> {
        let sent = self.sent_at?;
        Some([
            (Stage::Encode, self.encode_us as i64),
            (Stage::Send, self.queue_us as i64),
            (Stage::Net, self.first_arrival - sent),
            (Stage::Spread, self.last_arrival - self.first_arrival),
        ])
    }
}

/// 一次授时的结果。`offset_us` = 被控端时钟 − 主控端时钟。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockEstimate {
    pub rtt_us: i64,
    pub offset_us: i64,
}

/// 主控端对时：每次心跳带出本端时刻 t1，被控端回「收到 t2、回包 t3」，本端收到回包时 t4。
/// 标准 NTP 算法：往返 = (t4 − t1) − (t3 − t2)，时钟差 = ((t2 − t1) + (t3 − t4)) / 2，误差不超过往返的一半。
/// 用最近 [`SYNC_WINDOW_US`] 里往返最小的那次——它在路上排队最少，两个方向最对称。
#[derive(Default)]
pub struct ClockSync {
    samples: VecDeque<(i64, ClockEstimate)>,
}

impl ClockSync {
    pub fn on_echo(&mut self, t1: i64, t2: i64, t3: i64, t4: i64) {
        let rtt_us = (t4 - t1) - (t3 - t2);
        if rtt_us < 0 || t4 < t1 {
            // 本端时间倒退或回包对不上（不是我们发的那次），不用
            return;
        }
        let offset_us = ((t2 - t1) + (t3 - t4)) / 2;
        if self.samples.len() == SYNC_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back((t4, ClockEstimate { rtt_us, offset_us }));
    }

    /// `now` 之前 [`SYNC_WINDOW_US`] 内往返最小的一次；窗口里样本不足 [`SYNC_MIN_SAMPLES`] 时 None。
    pub fn estimate(&self, now: i64) -> Option<ClockEstimate> {
        let recent = || self.samples.iter().filter(|(at, _)| now - at <= SYNC_WINDOW_US);
        if recent().count() < SYNC_MIN_SAMPLES {
            return None;
        }
        recent().map(|(_, e)| *e).min_by_key(|e| e.rtt_us)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// 被控端：拿到画面 → 编码完成（含颜色转换）
    Encode,
    /// 被控端：编码完成 → 首包发出
    Send,
    /// 首包发出 → 首包到达（跨机器，靠对时）
    Net,
    /// 主控端：首包到达 → 末包到达
    Spread,
    /// 主控端：末包到达 → 解码完成（等 GPU 画完）
    Decode,
    /// 主控端：解码完成 → 颜色转换 + 缩放完成
    Convert,
    /// 主控端：转换完成 → 交给合成器
    Submit,
    /// 主控端：交给合成器 → 合成器报告开始显示
    Display,
    /// 拿到画面 → 最后一段的终点，直接用两端时刻算（和分段之和对账）
    Total,
}

const STAGES: [Stage; 9] = [
    Stage::Encode,
    Stage::Send,
    Stage::Net,
    Stage::Spread,
    Stage::Decode,
    Stage::Convert,
    Stage::Submit,
    Stage::Display,
    Stage::Total,
];

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Stage::Encode => "encode",
            Stage::Send => "send",
            Stage::Net => "net",
            Stage::Spread => "spread",
            Stage::Decode => "decode",
            Stage::Convert => "convert",
            Stage::Submit => "submit",
            Stage::Display => "display",
            Stage::Total => "total",
        }
    }
}

/// 分段之和与直接算的总数相差超过这么多，说明某一段的起止点没接上（代码错了），单独计数。
const MISMATCH_US: i64 = 100;

/// 按段累计一个统计窗口（调用方每隔几秒取走一次）。负值照记不丢：网络一段出现负数说明对时有误差，
/// 要让它露出来，不能悄悄扔掉。
#[derive(Default)]
pub struct Ledger {
    samples: [Vec<i64>; STAGES.len()],
    frames: u64,
    mismatches: u64,
}

impl Ledger {
    /// 记一帧：各段耗时和直接算出的总数。
    pub fn add_frame(&mut self, stages: &[(Stage, i64)], total: i64) {
        let mut sum = 0;
        for &(stage, us) in stages {
            self.samples[stage as usize].push(us);
            sum += us;
        }
        self.samples[Stage::Total as usize].push(total);
        if (sum - total).abs() > MISMATCH_US {
            self.mismatches += 1;
        }
        self.frames += 1;
    }

    /// 取走当前窗口的统计并清零。
    pub fn take(&mut self) -> LedgerSummary {
        let stages = STAGES
            .iter()
            .filter_map(|&s| {
                let v = &mut self.samples[s as usize];
                if v.is_empty() {
                    return None;
                }
                v.sort_unstable();
                let pick = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
                let summary = StageSummary {
                    stage: s,
                    avg_us: v.iter().sum::<i64>() / v.len() as i64,
                    p50_us: pick(0.5),
                    p95_us: pick(0.95),
                    max_us: v[v.len() - 1],
                    negatives: v.iter().filter(|&&x| x < 0).count(),
                };
                v.clear();
                Some(summary)
            })
            .collect();
        let summary = LedgerSummary {
            frames: self.frames,
            mismatches: self.mismatches,
            stages,
        };
        self.frames = 0;
        self.mismatches = 0;
        summary
    }
}

#[derive(Debug, Clone)]
pub struct StageSummary {
    pub stage: Stage,
    pub avg_us: i64,
    pub p50_us: i64,
    pub p95_us: i64,
    pub max_us: i64,
    pub negatives: usize,
}

#[derive(Debug, Clone)]
pub struct LedgerSummary {
    pub frames: u64,
    /// 分段之和与总数对不上的帧数，应为 0
    pub mismatches: u64,
    pub stages: Vec<StageSummary>,
}

impl LedgerSummary {
    pub fn stage(&self, stage: Stage) -> Option<&StageSummary> {
        self.stages.iter().find(|s| s.stage == stage)
    }
}

/// 一行：`frames N; encode p50/p95/max ms; ...`，负值个数和对账失败只在非零时出现。
impl fmt::Display for LedgerSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |us: i64| us as f64 / 1000.0;
        write!(f, "frames {}", self.frames)?;
        for s in &self.stages {
            write!(
                f,
                "; {} {:.2}/{:.2}/{:.2}",
                s.stage.name(),
                ms(s.p50_us),
                ms(s.p95_us),
                ms(s.max_us)
            )?;
            if s.negatives > 0 {
                write!(f, " ({} negative)", s.negatives)?;
            }
        }
        write!(f, " (ms p50/p95/max)")?;
        if self.mismatches > 0 {
            write!(f, "; {} frame(s) where the stages do not add up to the total", self.mismatches)?;
        }
        Ok(())
    }
}
