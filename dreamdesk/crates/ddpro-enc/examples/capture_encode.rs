//! 第 3 步验证工具：用 RustDesk 的采集模块拿真实桌面画面，NVENC 编成 HEVC 4:4:4，存码流并出统计。
//!
//!   capture_encode probe
//!       列出显示器和所在显卡，探测每块 N 卡的编码能力
//!   capture_encode run --out DIR [--seconds 10] [--fps 60] [--bitrate 30000] [--preset 4]
//!                      [--dither 1] [--ref-frame 30]
//!       编 N 秒：DIR\out.h265 码流、DIR\stats.txt 统计；
//!       第 ref-frame 帧的原始画面存 DIR\ref_<宽>x<高>.bgra，用于和解码结果比颜色
//!   capture_encode compare REF.bgra DEC.rgb WIDTH HEIGHT
//!       原始画面（BGRA）和 ffmpeg 解出的同一帧（rgb24）逐像素比，报每通道最大误差、平均误差、PSNR
//!
//! 必须在有人登录的桌面会话里跑：远程管理通道那种后台会话采集不到屏幕。

#[cfg(not(windows))]
fn main() {
    eprintln!("capture_encode needs Windows with an NVIDIA GPU");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("probe") => win::probe(),
        Some("run") => win::run(&args[1..]),
        Some("compare") => win::compare(&args[1..]),
        _ => Err("usage: capture_encode probe | run --out DIR [...] | compare REF.bgra DEC.rgb W H".into()),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod win {
    use ddpro_enc::{Config, Encoder};
    // 用底层的 DXGI 采集器：通用接口上「输出纹理 / 取设备」要开 RustDesk 的 vram 功能，
    // 那会改变通用模式的编码路径，不能为了我们去开
    use scrap::{
        dxgi::{Capturer, Display, Displays},
        Frame,
    };
    use std::{
        fs,
        io::{ErrorKind, Write},
        path::PathBuf,
        time::{Duration, Instant},
    };

    type Res = Result<(), Box<dyn std::error::Error>>;
    const NVIDIA: u32 = 0x10de;

    fn describe(d: &Display) -> String {
        let name = String::from_utf16_lossy(d.name());
        format!("{} {}x{}", name.trim_end_matches('\0'), d.width(), d.height())
    }

    pub fn probe() -> Res {
        for d in Displays::new()? {
            let desc = describe(&d);
            match Capturer::new(d) {
                Ok(c) => {
                    let dev = c.device();
                    let caps = (dev.vendor_id == NVIDIA).then(|| ddpro_enc::probe(dev.device));
                    println!("{desc} vendor={:#06x} caps={caps:?}", dev.vendor_id);
                }
                Err(e) => println!("{desc} capturer error: {e}"),
            }
        }
        Ok(())
    }

    struct Opts {
        out: PathBuf,
        seconds: u32,
        fps: u32,
        bitrate: u32,
        preset: u8,
        dither: bool,
        ref_frame: u32,
    }

    fn parse(args: &[String]) -> Result<Opts, Box<dyn std::error::Error>> {
        let mut o = Opts {
            out: PathBuf::new(),
            seconds: 10,
            fps: 60,
            bitrate: 30_000,
            preset: 4,
            dither: true,
            ref_frame: 30,
        };
        let mut it = args.iter();
        while let Some(k) = it.next() {
            let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
            match k.as_str() {
                "--out" => o.out = PathBuf::from(v),
                "--seconds" => o.seconds = v.parse()?,
                "--fps" => o.fps = v.parse()?,
                "--bitrate" => o.bitrate = v.parse()?,
                "--preset" => o.preset = v.parse()?,
                "--dither" => o.dither = v != "0",
                "--ref-frame" => o.ref_frame = v.parse()?,
                _ => return Err(format!("unknown option {k}").into()),
            }
        }
        if o.out.as_os_str().is_empty() {
            return Err("--out DIR is required".into());
        }
        Ok(o)
    }

    /// 第一块在 N 卡上的显示器：编码器必须和采集用同一块卡上的设备。
    fn nvidia_capturer() -> Result<(Capturer, u32, u32), Box<dyn std::error::Error>> {
        for d in Displays::new()? {
            let desc = describe(&d);
            let (w, h) = (d.width() as u32, d.height() as u32);
            let mut c = Capturer::new(d)?;
            if c.device().vendor_id == NVIDIA {
                c.set_output_texture(true);
                println!("capturing {desc}");
                return Ok((c, w, h));
            }
        }
        Err("no display on an NVIDIA GPU".into())
    }

    pub fn run(args: &[String]) -> Res {
        let o = parse(args)?;
        fs::create_dir_all(&o.out)?;
        let (mut cap, w, h) = nvidia_capturer()?;
        let cfg = Config {
            width: w,
            height: h,
            fps: o.fps,
            bitrate_kbps: o.bitrate,
            preset: o.preset,
            dither: o.dither,
        };
        println!("{cfg:?}");
        let mut enc = Encoder::new(cap.device().device, cfg)?;
        let mut stream = fs::File::create(o.out.join("out.h265"))?;

        let total = o.seconds * o.fps;
        let interval = Duration::from_secs_f64(1.0 / o.fps as f64);
        let (mut enc_ms, mut sizes) = (Vec::new(), Vec::new());
        let (mut new_frames, mut idr) = (0u32, 0u32);
        let start = Instant::now();
        for i in 0..total {
            let tick = start + interval * i;
            // 等新画面最多到这一帧的时刻；没有新画面就重编上一帧（静止时仍按帧率出帧）
            let wait = tick.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
            let src = match cap.frame(wait.as_millis() as u32) {
                Ok(Frame::Texture((t, _))) if !t.is_null() => Some(t),
                Ok(_) => None,
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => None,
                Err(e) => return Err(format!("capture failed at frame {i}: {e}").into()),
            };
            if i == 0 && src.is_none() {
                // 第一帧必须有画面：DXGI 开始采集后第一次取帧总会给出整屏
                return Err("no first frame from the capturer".into());
            }
            new_frames += src.is_some() as u32;
            let t0 = Instant::now();
            let f = enc.encode(src, false)?;
            enc_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            sizes.push(f.data.len());
            idr += f.is_idr as u32;
            stream.write_all(&f.data)?;
            if i == o.ref_frame {
                fs::write(o.out.join(format!("ref_{w}x{h}.bgra")), enc.read_source_bgra()?)?;
            }
            let next = start + interval * (i + 1);
            if let Some(d) = next.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
        }
        let wall = start.elapsed().as_secs_f64();

        let mut sorted = enc_ms.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
        let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let size_avg = sizes.iter().sum::<usize>() as f64 / sizes.len() as f64;
        let report = format!(
            "frames {total} in {wall:.2}s ({:.1} fps), new captured frames {new_frames}, idr {idr}\n\
             encode ms: avg {:.2} p50 {:.2} p95 {:.2} max {:.2}\n\
             frame bytes: avg {:.0} max {} (first {})\n\
             bitrate actual {:.1} Mbps\n\
             ref frame {} -> ref_{w}x{h}.bgra\n",
            total as f64 / wall,
            avg(&enc_ms),
            pct(0.5),
            pct(0.95),
            sorted[sorted.len() - 1],
            size_avg,
            sizes.iter().max().unwrap_or(&0),
            sizes[0],
            sizes.iter().sum::<usize>() as f64 * 8.0 / wall / 1e6,
            o.ref_frame,
        );
        print!("{report}");
        fs::write(o.out.join("stats.txt"), report)?;
        Ok(())
    }

    pub fn compare(args: &[String]) -> Res {
        let [r, d, w, h] = args else {
            return Err("compare REF.bgra DEC.rgb WIDTH HEIGHT".into());
        };
        let (w, h): (usize, usize) = (w.parse()?, h.parse()?);
        let (r, d) = (fs::read(r)?, fs::read(d)?);
        if r.len() != w * h * 4 || d.len() != w * h * 3 {
            return Err(format!("size mismatch: ref {} bytes, dec {} bytes for {w}x{h}", r.len(), d.len()).into());
        }
        let (mut max, mut sum, mut sq) = ([0u8; 3], [0u64; 3], 0u64);
        for i in 0..w * h {
            // ref 是 BGRA，dec 是 RGB
            let rgb_ref = [r[i * 4 + 2], r[i * 4 + 1], r[i * 4]];
            for c in 0..3 {
                let e = rgb_ref[c].abs_diff(d[i * 3 + c]);
                max[c] = max[c].max(e);
                sum[c] += e as u64;
                sq += (e as u64) * (e as u64);
            }
        }
        let n = (w * h) as f64;
        let mse = sq as f64 / (n * 3.0);
        let psnr = if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0 * 255.0 / mse).log10() };
        println!(
            "max abs error R/G/B: {}/{}/{}\nmean abs error R/G/B: {:.3}/{:.3}/{:.3}\nPSNR: {psnr:.2} dB",
            max[0],
            max[1],
            max[2],
            sum[0] as f64 / n,
            sum[1] as f64 / n,
            sum[2] as f64 / n
        );
        Ok(())
    }
}
