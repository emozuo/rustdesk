//! 第 5 步验证工具（瘦客户机的 Wayland 桌面里跑）：录好的码流循环解码 → 转 RGB → 全屏窗口零拷贝显示，
//! 统计分段耗时、上屏情况和 CPU 占用。
//!
//!   view_check IN.h265 [--seconds 60]
//!
//! - 开全屏窗口，报屏幕尺寸、合成器能不能用 / 直接送显示我们的格式（XRGB8888，线性排布）；
//! - 按 60 帧/秒循环解码，转成屏幕大小的 RGB，提交给合成器；
//! - 每 5 秒一行：解码 / 转换 / 提交 / 上屏 各段 p50/p95，合成器直接送显示的比例、跳过的帧、没有空闲画面而丢的帧，
//!   本进程和 gnome-shell 的 CPU 占用（第 6 步看合成器会不会越跑越高）。

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("view_check runs on Linux only");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(e) = linux::run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use ddpro_dec::{Converter, Decoder, Device, RgbStandard};
    use ddpro_media::hevc;
    use ddpro_view::{now_ns, Event, Window};
    use std::{
        collections::HashMap,
        fs,
        time::{Duration, Instant},
    };

    const SLOTS: usize = 4;
    /// DRM 格式 XRGB8888（'XR24'）
    const XRGB8888: u32 = 0x3432_5258;
    const FRAME: Duration = Duration::from_nanos(16_666_667);
    const REPORT: Duration = Duration::from_secs(5);

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let input = args.first().ok_or("usage: view_check IN.h265 [--seconds N]")?;
        let seconds: u64 = match args.get(1).map(String::as_str) {
            Some("--seconds") => args.get(2).ok_or("--seconds needs a value")?.parse()?,
            _ => 60,
        };
        let aus = hevc::split_access_units(&fs::read(input)?);
        println!("{} access units from {input}, running {seconds} s", aus.len());

        let mut win = Window::open("dreamdesk-pro view check")?;
        let (w, h) = win.size();
        println!("window: {w}x{h} (fullscreen), keeping the screen awake: {}", win.idle_inhibited());
        match win.formats() {
            Some(f) => println!(
                "compositor: XRGB8888 linear usable: {}, direct scanout possible: {} ({} usable / {} scanout pairs)",
                f.usable.contains(&(XRGB8888, 0)),
                f.scanout.contains(&(XRGB8888, 0)),
                f.usable.len(),
                f.scanout.len()
            ),
            None => println!("compositor: no per-surface format feedback (linux-dmabuf < v4)"),
        }

        let dev = Device::open()?;
        let mut dec = Decoder::new(&dev)?;
        let mut conv = Converter::new(&dev, w, h, SLOTS)?;
        win.reset_slots(SLOTS);
        for slot in 0..SLOTS {
            win.import(slot, &conv.export(slot)?)?;
        }

        let mut stats = Stats::default();
        let mut cpu = Cpu::new();
        let start = Instant::now();
        let mut next = start;
        let mut last_report = start;
        let mut i = 0usize;
        while start.elapsed() < Duration::from_secs(seconds) {
            // 到点出下一帧；没到点就处理合成器的事件
            let wait = next.saturating_duration_since(Instant::now());
            for e in win.dispatch(wait)? {
                match e {
                    Event::Presented { commit_ns, present_ns, zero_copy, foreign_clock, .. } => {
                        stats.presented += 1;
                        stats.zero_copy += zero_copy as u64;
                        if !foreign_clock {
                            stats.display.push((present_ns - commit_ns) as f64 / 1e6);
                        }
                    }
                    Event::Discarded { .. } => stats.discarded += 1,
                    Event::Resized { width, height } => println!("resized to {width}x{height} (not handled by this tool)"),
                    Event::Closed => return Ok(()),
                }
            }
            if Instant::now() < next {
                continue;
            }
            next += FRAME;

            let t0 = now_ns();
            let au = &aus[i % aus.len()];
            i += 1;
            let Some(frame) = dec.decode(au)? else { continue };
            dec.wait(&frame)?;
            let t1 = now_ns();
            let Some(slot) = win.free_slot() else {
                // 合成器拿着所有画面：这一帧丢掉（解码照做，参考链不断）
                stats.no_slot += 1;
                continue;
            };
            conv.convert(&frame, slot, RgbStandard::Bt709)?;
            conv.wait(slot)?;
            let t2 = now_ns();
            win.present(slot)?;
            let t3 = now_ns();
            stats.decode.push((t1 - t0) as f64 / 1e6);
            stats.convert.push((t2 - t1) as f64 / 1e6);
            stats.submit.push((t3 - t2) as f64 / 1e6);
            stats.frames += 1;

            if last_report.elapsed() >= REPORT {
                last_report = Instant::now();
                println!("[{:>4.0}s] {} | {}", start.elapsed().as_secs_f64(), stats.take(), cpu.sample());
            }
        }
        println!("[end] {} | {}", stats.take(), cpu.sample());
        Ok(())
    }

    #[derive(Default)]
    struct Stats {
        frames: u64,
        presented: u64,
        zero_copy: u64,
        discarded: u64,
        no_slot: u64,
        decode: Vec<f64>,
        convert: Vec<f64>,
        submit: Vec<f64>,
        display: Vec<f64>,
    }

    impl Stats {
        fn take(&mut self) -> String {
            let s = std::mem::take(self);
            format!(
                "frames {} presented {} (direct scanout {}) discarded {} no-free-slot {} | ms p50/p95: decode {} convert {} submit {} display {}",
                s.frames,
                s.presented,
                s.zero_copy,
                s.discarded,
                s.no_slot,
                pct(s.decode),
                pct(s.convert),
                pct(s.submit),
                pct(s.display)
            )
        }
    }

    fn pct(mut v: Vec<f64>) -> String {
        if v.is_empty() {
            return "-".into();
        }
        v.sort_by(|a, b| a.total_cmp(b));
        let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        format!("{:.2}/{:.2}", p(0.5), p(0.95))
    }

    /// 本进程和 gnome-shell 在两次采样之间的 CPU 占用（单核 = 100%）。
    struct Cpu {
        at: Instant,
        last: HashMap<u32, u64>,
        shell: Option<u32>,
        ticks: f64,
    }

    impl Cpu {
        fn new() -> Self {
            let shell = fs::read_dir("/proc").ok().and_then(|d| {
                d.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
                    .find(|pid| fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "gnome-shell"))
            });
            let mut c = Self {
                at: Instant::now(),
                last: HashMap::new(),
                shell,
                ticks: unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64,
            };
            c.sample();
            c
        }

        fn ticks_of(pid: u32) -> Option<u64> {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            // 第 2 列是带括号的进程名，可能含空格：从右括号之后数
            let rest = &stat[stat.rfind(')')? + 2..];
            let f: Vec<&str> = rest.split_whitespace().collect();
            Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
        }

        fn sample(&mut self) -> String {
            let secs = self.at.elapsed().as_secs_f64().max(1e-3);
            self.at = Instant::now();
            let (last, ticks, shell) = (&mut self.last, self.ticks, self.shell);
            let mut pct = |pid: u32| -> Option<f64> {
                let now = Self::ticks_of(pid)?;
                let prev = last.insert(pid, now).unwrap_or(now);
                Some((now - prev) as f64 / ticks / secs * 100.0)
            };
            let me = pct(std::process::id()).map_or("-".into(), |v| format!("{v:.0}%"));
            let shell = shell.and_then(&mut pct).map_or("-".into(), |v| format!("{v:.0}%"));
            format!("cpu: this {me}, gnome-shell {shell}")
        }
    }
}
