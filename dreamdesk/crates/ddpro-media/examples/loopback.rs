//! 第 4 步自测：在本机用录好的真实 H.265 码流走一遍完整通道（UDP + RTP + SRTP），不需要显卡。
//!
//!   loopback IN.h265 [--drop 千分比] [--pace KBPS] [--out RECEIVED.h265]
//!
//! - 发送端按 60 帧/秒发出每一帧，发包节奏按 --pace 的码率（默认 30000，和专业模式一样；0 = 不控节奏）；
//!   接收端缺包先请求补发，补发也等不到才回发「请求关键帧」，发送端收到后用重发第一帧（关键帧）代替编码器出关键帧；
//! - 没有救不回来的丢包时（不丢包，或丢的全补回来了）检查收到的和发出的逐个 NAL 完全一致；
//!   否则统计「发现丢包 → 收到关键帧」的耗时；
//! - 发送端发完就停，接收端应在 3 秒后报通道断开；
//! - 分段延迟（第 5a 步）：两端在同一台机器上，真实的时钟差已知（两端时间轴零点之差），
//!   检查授时算出的时钟差误差 ≤ 0.1 ms、各段之和与总数对得上、网络一段没有负数。

use ddpro_media::{hevc, Event, Keys, Ledger, Listener, Receiver, Stamp};
use std::{
    net::{IpAddr, Ipv4Addr},
    time::{Duration, Instant},
};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// 自测没有真的采集和编码：拿到画面、编码完成都记成现在
fn stamp_now() -> Stamp {
    let now = Instant::now();
    Stamp {
        captured: now,
        encoded: now,
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let input = args.first().ok_or("usage: loopback IN.h265 [--drop PERMILLE] [--pace KBPS] [--out FILE]")?;
    let mut drop = 0u32;
    let mut pace = 30_000u32;
    let mut out = None;
    let mut it = args[1..].iter();
    while let Some(k) = it.next() {
        let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
        match k.as_str() {
            "--drop" => drop = v.parse()?,
            "--pace" => pace = v.parse()?,
            "--out" => out = Some(v.clone()),
            _ => return Err(format!("unknown option {k}").into()),
        }
    }

    let aus = hevc::split_access_units(&std::fs::read(input)?);
    let first_irap = hevc::split_annexb(&aus[0]).into_iter().any(hevc::is_irap);
    println!("{} frames from {input}, first frame is keyframe: {first_irap}", aus.len());
    if !first_irap {
        return Err("the stream must start with a keyframe".into());
    }

    let keys = Keys::generate();
    let listener = Listener::bind(IpAddr::V4(Ipv4Addr::LOCALHOST), keys.clone())?;
    let addr = listener.local_addr()?;
    let sender_epoch = listener.epoch();
    let sent = aus.clone();
    let sender = std::thread::spawn(move || -> Result<u64, String> {
        let mut s = listener.accept(Duration::from_secs(3)).map_err(|e| e.to_string())?;
        if pace > 0 {
            s.pace(pace, 60);
        }
        let start = Instant::now();
        let mut keyframes = 0u64;
        for (i, au) in sent.iter().enumerate() {
            // 有关键帧请求就先补发一次关键帧（代替编码器按需出关键帧）
            if s.take_keyframe_request() {
                s.send_frame(&sent[0], (i as u32) * 1500, Some(stamp_now())).map_err(|e| e.to_string())?;
                keyframes += 1;
            }
            s.send_frame(au, (i as u32) * 1500, Some(stamp_now())).map_err(|e| e.to_string())?;
            if !s.peer_alive() {
                return Err("controller keepalive lost".into());
            }
            let next = start + Duration::from_secs_f64((i + 1) as f64 / 60.0);
            if let Some(d) = next.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
        }
        println!("sender: {:?}, keyframes resent on request: {keyframes}", s.stats);
        Ok(keyframes)
    });

    let mut r = Receiver::connect(addr, keys, Duration::from_secs(3), drop)?;
    // 被控端时钟 − 主控端时钟 = 主控端零点 − 被控端零点
    let true_offset = r.epoch().duration_since(sender_epoch).as_micros() as i64;
    let mut ledger = Ledger::default();
    let mut clock_err_max = 0i64;
    let mut untimed = 0u64;
    let mut received = Vec::new();
    let mut loss_at: Option<Instant> = None;
    let mut recoveries_ms = Vec::new();
    let gone = loop {
        match r.recv() {
            Ok(Event::Frame { data, times, .. }) => {
                if let Some(t) = loss_at.take() {
                    recoveries_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                match times.and_then(|t| Some((t.transport_stages()?, t.captured_at()?, t.last_arrival))) {
                    Some((stages, captured, end)) => ledger.add_frame(&stages, end - captured),
                    None => untimed += 1,
                }
                if let Some(c) = r.clock() {
                    clock_err_max = clock_err_max.max((c.offset_us - true_offset).abs());
                }
                received.push(data);
            }
            Ok(Event::Loss { .. }) => {
                loss_at.get_or_insert_with(Instant::now);
            }
            Err(e) => break e,
        }
    };
    let sender_result = sender.join().map_err(|_| "sender panicked")?;
    println!("receiver ended with: {gone}");
    println!("sender result: {sender_result:?}");
    println!("receiver: {:?}", r.stats);

    let summary = ledger.take();
    println!("latency: {summary}");
    println!(
        "clock: true offset {true_offset} us, last estimate {:?}, max error {clock_err_max} us; frames without timing {untimed}",
        r.clock()
    );
    // 网络一段和帧内跨度都不能有负数：补发时暂存的包按到达时刻算首包、按拼进来的时刻算收齐，算错了就会出负数
    let negatives = |stage| summary.stage(stage).map_or(0, |s| s.negatives);
    let net_negatives = negatives(ddpro_media::Stage::Net) + negatives(ddpro_media::Stage::Spread);
    if clock_err_max > 100 || summary.mismatches > 0 || net_negatives > 0 || summary.frames == 0 {
        return Err("latency ledger check failed".into());
    }

    if let Some(path) = out {
        std::fs::write(&path, received.concat())?;
        println!("received stream written to {path}");
    }
    if r.stats.loss_events == 0 {
        // 逐个 NAL 比（起始码 3 / 4 字节的差别不算）
        let a: Vec<&[u8]> = aus.iter().flat_map(|f| hevc::split_annexb(f)).collect();
        let joined = received.concat();
        let b = hevc::split_annexb(&joined);
        let same = a == b;
        println!("lossless check: sent {} NALs, received {} NALs, identical: {same}", a.len(), b.len());
        if !same {
            return Err("received stream differs from sent stream".into());
        }
    } else {
        recoveries_ms.sort_by(|x, y| x.total_cmp(y));
        let p = |q: f64| recoveries_ms.get(((recoveries_ms.len().max(1) - 1) as f64 * q) as usize).copied().unwrap_or(0.0);
        println!(
            "recoveries {}: loss -> keyframe ms p50 {:.1} p95 {:.1} max {:.1}; frames received {} of {}",
            recoveries_ms.len(),
            p(0.5),
            p(0.95),
            p(1.0),
            received.len(),
            aus.len()
        );
    }
    Ok(())
}
