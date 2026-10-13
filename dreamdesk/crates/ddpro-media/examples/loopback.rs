//! 第 4 步自测：在本机用录好的真实 H.265 码流走一遍完整通道（UDP + RTP + SRTP），不需要显卡。
//!
//!   loopback IN.h265 [--drop 千分比] [--out RECEIVED.h265]
//!
//! - 发送端按 60 帧/秒发出每一帧；接收端发现丢包时「请求关键帧」，发送端用重发第一帧（关键帧）代替编码器
//!   出关键帧；
//! - 不丢包时检查收到的和发出的逐个 NAL 完全一致；丢包时统计「发现丢包 → 收到关键帧」的耗时；
//! - 发送端发完就停，接收端应在 3 秒后报通道断开。

use ddpro_media::{hevc, Event, Keys, Listener, Receiver};
use std::{
    net::{IpAddr, Ipv4Addr},
    sync::mpsc,
    time::{Duration, Instant},
};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let input = args.first().ok_or("usage: loopback IN.h265 [--drop PERMILLE] [--out FILE]")?;
    let mut drop = 0u32;
    let mut out = None;
    let mut it = args[1..].iter();
    while let Some(k) = it.next() {
        let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
        match k.as_str() {
            "--drop" => drop = v.parse()?,
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
    let (kf_tx, kf_rx) = mpsc::channel::<()>();
    let sent = aus.clone();
    let sender = std::thread::spawn(move || -> Result<u64, String> {
        let mut s = listener.accept(Duration::from_secs(3)).map_err(|e| e.to_string())?;
        let start = Instant::now();
        let mut keyframes = 0u64;
        for (i, au) in sent.iter().enumerate() {
            // 有关键帧请求就先补发一次关键帧（代替编码器按需出关键帧）
            if kf_rx.try_iter().count() > 0 {
                s.send_frame(&sent[0], (i as u32) * 1500).map_err(|e| e.to_string())?;
                keyframes += 1;
            }
            s.send_frame(au, (i as u32) * 1500).map_err(|e| e.to_string())?;
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
    let mut received = Vec::new();
    let mut loss_at: Option<Instant> = None;
    let mut recoveries_ms = Vec::new();
    let gone = loop {
        match r.recv() {
            Ok(Event::Frame { data, .. }) => {
                if let Some(t) = loss_at.take() {
                    recoveries_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                received.push(data);
            }
            Ok(Event::Loss { .. }) => {
                loss_at.get_or_insert_with(Instant::now);
                let _ = kf_tx.send(());
            }
            Err(e) => break e,
        }
    };
    let sender_result = sender.join().map_err(|_| "sender panicked")?;
    println!("receiver ended with: {gone}");
    println!("sender result: {sender_result:?}");
    println!("receiver: {:?}", r.stats);

    if let Some(path) = out {
        std::fs::write(&path, received.concat())?;
        println!("received stream written to {path}");
    }
    if drop == 0 {
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
