//! 第 3 步验证工具：用 RustDesk 的采集模块拿真实桌面画面，NVENC 编成 HEVC 4:4:4，存码流并出统计。
//!
//!   capture_encode probe
//!       列出显示器和所在显卡，探测每块 N 卡的编码能力
//!   capture_encode run --out DIR [--seconds 10] [--fps 60] [--bitrate 30000] [--preset 4]
//!                      [--dither 0] [--ref-frame 30]
//!       编 N 秒：DIR\out.h265 码流、DIR\stats.txt 统计；
//!       第 ref-frame 帧的原始画面存 DIR\ref_<宽>x<高>.bgra，用于和解码结果比颜色
//!   capture_encode compare REF.bgra DEC.rgb WIDTH HEIGHT
//!       原始画面（BGRA）和 ffmpeg 解出的同一帧（rgb24）逐像素比，报每通道最大误差、平均误差、PSNR
//!   capture_encode synth --out DIR [--move-seconds 2] [--still-seconds 3] [--speed 8]
//!                        [--fps 60] [--bitrate 30000] [--preset 4] [--dither 0] [--noise 0] [--bits 8|10]
//!       P1 静止收敛：工具自己生成 CG 类测试图（色卡、渐变、1 像素细线、彩色棋盘、彩色小字），先平移 N 秒、再完全静止
//!       N 秒，按专业模式的方式编码（静止时不给新画面、编码器重编上一帧）。出 DIR\out.h265、DIR\frames.csv、DIR\synth.txt。
//!       --noise A 在运动段每帧叠加 ±A 的随机噪声（剧烈运动，P1 实测用 24）；不加时纯平移，码控一直压得很低，测不出收敛
//!   capture_encode curve DIR < 解码出的 rgb24 帧流
//!       逐帧和原图比（原图按 synth.txt 重画），出 DIR\curve.csv 和静止段的汇总：多久回到颜色换算下限、
//!       最终画质、有没有回退（闪）、静止段最大单帧。解码：
//!       ffmpeg -i DIR\out.h265 -vf "scale=in_range=pc:in_color_matrix=bt709:out_range=pc:flags=accurate_rnd+full_chroma_int+bitexact,format=rgb24" -f rawvideo - | capture_encode curve DIR
//!
//!   capture_encode hdr --out DIR [--seconds 2] [--sdr-white 240] [--bitrate 30000] [--preset 4]
//!       P1 第 4 项 HDR：工具自己生成一张 HDR 桌面那样的 scRGB（FP16）测试图——上半部分是 CG 测试图按「SDR 白 = N 尼特」放进去
//!       （普通内容在 HDR 桌面上的样子），下半部分是 0–1000 尼特的灰阶和高光色块（含 BT.709 装不下的颜色、超过 10000 尼特的白）——
//!       静止编 N 秒（BT.2020 + PQ、4:4:4 10bit）。出 DIR\out.h265、DIR\expected.yuv（CPU 按着色器公式算的期望值，
//!       yuv444p10le 一帧）、DIR\hdr.txt（宽 高）
//!   capture_encode hdrcheck DIR < 解码出的 yuv444p10le 帧流
//!       第一帧和最后一帧逐像素和期望值比（10 位级差），分区汇总。解码：
//!       ffmpeg -i DIR\out.h265 -f rawvideo -pix_fmt yuv444p10le - | capture_encode hdrcheck DIR
//!   capture_encode hdrrgb DIR SHADER.rgb
//!       主控端着色器（瘦客户机 `vaapi_check out.h265 --gl --hdr N`）读回的 RGB 和期望值比：期望值是 DIR\expected.yuv 按主控端
//!       着色器的公式（ddpro-dec gl.rs，PQ 还原、除以普通内容的白、BT.2020→BT.709、高光保色相截到白、sRGB）算的；
//!       另外上半部分（普通内容）直接和原来的 8bit CG 测试图比——HDR 桌面上的普通内容应当显示得和普通桌面一样
//!
//! probe / run / synth / hdr 必须在有人登录的桌面会话里跑：远程管理通道那种后台会话采集不到屏幕（synth 只借采集器拿 N 卡的设备）。
//! compare / curve 不碰显卡，哪里都能跑。

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
        Some("synth") => win::synth(&args[1..]),
        Some("curve") => win::curve(&args[1..]),
        Some("hdr") => win::hdr(&args[1..]),
        Some("hdrcheck") => win::hdr_check(&args[1..]),
        Some("hdrrgb") => win::hdr_rgb(&args[1..]),
        _ => Err("usage: capture_encode probe | run --out DIR [...] | compare REF.bgra DEC.rgb W H | synth --out DIR [...] | curve DIR | hdr --out DIR [...] | hdrcheck DIR".into()),
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
            dither: false,
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
            tile: 0,
            ten_bit: false,
            hdr: false,
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

    /// 「先动后停」场景的参数：synth 写进 DIR\synth.txt，curve 读回来按同样的参数重画原图。
    struct Synth {
        w: u32,
        h: u32,
        fps: u32,
        bitrate: u32,
        move_frames: u32,
        still_frames: u32,
        speed: u32,
        dither: bool,
        /// 运动段每帧叠加的随机噪声幅度（0 = 不加）。纯平移能被运动矢量完美预测、码控一直压得很低，
        /// 加噪声模拟「拖动大图、滚动网页」这类预测不了的运动，停下时画面才真是糊的
        noise: u32,
    }

    impl Synth {
        fn save(&self, path: PathBuf) -> std::io::Result<()> {
            let s = self;
            fs::write(
                path,
                format!(
                    "{} {} {} {} {} {} {} {} {}\n",
                    s.w, s.h, s.fps, s.bitrate, s.move_frames, s.still_frames, s.speed, s.dither as u32, s.noise
                ),
            )
        }

        fn load(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
            let v: Vec<u32> = fs::read_to_string(path)?
                .split_whitespace()
                .map(str::parse)
                .collect::<Result<_, _>>()?;
            let [w, h, fps, bitrate, move_frames, still_frames, speed, dither, noise] = v[..] else {
                return Err("synth.txt needs 9 numbers".into());
            };
            Ok(Self { w, h, fps, bitrate, move_frames, still_frames, speed, dither: dither != 0, noise })
        }

        /// 第 i 帧的原图。噪声只加在运动段、最后一个运动帧之前：停下的那一帧是干净的测试图
        fn frame(&self, base: &[u8], i: u32, out: &mut [u8]) {
            shifted(base, self.w as usize, self.h as usize, self.offset(i), out);
            if self.noise == 0 || i + 1 >= self.move_frames {
                return;
            }
            let a = self.noise as i32;
            let mut seed = i.wrapping_mul(2_654_435_761).wrapping_add(1);
            for px in out.chunks_exact_mut(4) {
                for c in &mut px[..3] {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let n = (seed >> 16) as i32 % (2 * a + 1) - a;
                    *c = (*c as i32 + n).clamp(0, 255) as u8;
                }
            }
        }

        /// 第 i 帧的原图是测试图平移多少：运动段每帧向右走 speed、向下走 speed / 3 像素（循环卷动），之后停在最后的位置
        fn offset(&self, i: u32) -> (usize, usize) {
            let k = i.min(self.move_frames.saturating_sub(1)) as usize;
            let s = self.speed as usize;
            ((k * s) % self.w as usize, (k * s / 3) % self.h as usize)
        }
    }

    /// CG 类测试图（BGRA），从上到下四段：色卡（1 像素白格线）、红 / 绿 / 蓝 / 灰渐变、1 像素彩色竖线和最高频的
    /// 红蓝棋盘（4:2:0 会把它抹成紫色）、彩色小字形状（5×7 点阵，前景色 / 背景色随格子变）。固定随机种子，每次一样。
    fn test_pattern(w: usize, h: usize) -> Vec<u8> {
        const PALETTE: [[u8; 3]; 6] = [[220, 30, 30], [30, 180, 40], [40, 60, 220], [240, 200, 20], [200, 40, 200], [20, 190, 200]];
        let mut seed: u32 = 0x1234_5678;
        let glyphs: Vec<u64> = (0..64)
            .map(|_| {
                let mut g = 0u64;
                for _ in 0..35 {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    g = (g << 1) | (seed >> 31) as u64;
                }
                g
            })
            .collect();
        let band = (h / 4).max(1);
        let mut p = vec![0u8; w * h * 4];
        for y in 0..h {
            let yb = y % band;
            for x in 0..w {
                let rgb = match y / band {
                    0 => {
                        let (cw, ch) = ((w / 8).max(1), (band / 3).max(1));
                        if x % cw == 0 || yb % ch == 0 {
                            [255, 255, 255]
                        } else {
                            let c = PALETTE[(x / cw) % 6];
                            match yb / ch {
                                0 => c,
                                1 => [c[0] / 2 + 64, c[1] / 2 + 64, c[2] / 2 + 64],
                                _ => [c[0] / 3, c[1] / 3, c[2] / 3],
                            }
                        }
                    }
                    1 => {
                        let q = (w / 4).max(1);
                        let v = ((x % q) * 255 / q) as u8;
                        match x / q {
                            0 => [v, 0, 0],
                            1 => [0, v, 0],
                            2 => [0, 0, v],
                            _ => [v, v, v],
                        }
                    }
                    2 => {
                        if x < w / 2 {
                            PALETTE[x % 6]
                        } else if (x + y) % 2 == 0 {
                            [255, 0, 0]
                        } else {
                            [0, 0, 255]
                        }
                    }
                    _ => {
                        let cell = (x / 6) * 131 + (yb / 9) * 977;
                        let (cx, cy) = (x % 6, yb % 9);
                        let on = cx < 5 && cy < 7 && (glyphs[cell % 64] >> (cy * 5 + cx)) & 1 == 1;
                        if on {
                            PALETTE[cell % 6]
                        } else {
                            match cell % 3 {
                                0 => [255, 255, 255],
                                1 => [0, 0, 0],
                                _ => PALETTE[(cell + 3) % 6],
                            }
                        }
                    }
                };
                let i = (y * w + x) * 4;
                p[i..i + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
            }
        }
        p
    }

    /// 测试图循环平移 (dx, dy) 后的画面
    fn shifted(base: &[u8], w: usize, h: usize, (dx, dy): (usize, usize), out: &mut [u8]) {
        for y in 0..h {
            let src = &base[((y + dy) % h) * w * 4..][..w * 4];
            let dst = &mut out[y * w * 4..][..w * 4];
            dst[..(w - dx) * 4].copy_from_slice(&src[dx * 4..]);
            dst[(w - dx) * 4..].copy_from_slice(&src[..dx * 4]);
        }
    }

    pub fn synth(args: &[String]) -> Res {
        use windows::{
            core::Interface,
            Win32::Graphics::{Direct3D11::*, Dxgi::Common::*},
        };
        let (mut out, mut move_s, mut still_s, mut speed) = (PathBuf::new(), 2u32, 3u32, 8u32);
        let (mut fps, mut bitrate, mut preset, mut dither) = (60u32, 30_000u32, 4u8, false);
        let mut noise = 0u32;
        let mut ten_bit = false;
        let mut it = args.iter();
        while let Some(k) = it.next() {
            let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
            match k.as_str() {
                "--out" => out = PathBuf::from(v),
                "--move-seconds" => move_s = v.parse()?,
                "--still-seconds" => still_s = v.parse()?,
                "--speed" => speed = v.parse()?,
                "--noise" => noise = v.parse()?,
                "--fps" => fps = v.parse()?,
                "--bitrate" => bitrate = v.parse()?,
                "--preset" => preset = v.parse()?,
                "--dither" => dither = v != "0",
                "--bits" => ten_bit = v == "10",
                _ => return Err(format!("unknown option {k}").into()),
            }
        }
        if out.as_os_str().is_empty() {
            return Err("--out DIR is required".into());
        }
        fs::create_dir_all(&out)?;
        // 采集器只用来拿 N 卡上的设备（编码器要和画面在同一个设备上）；画面是自己生成的
        let (cap, w, h) = nvidia_capturer()?;
        let s = Synth { w, h, fps, bitrate, move_frames: move_s * fps, still_frames: still_s * fps, speed, dither, noise };
        s.save(out.join("synth.txt"))?;
        let (wu, hu) = (w as usize, h as usize);
        let base = test_pattern(wu, hu);
        let mut frame = vec![0u8; base.len()];

        let device = cap.device().device;
        let (tex, ctx) = unsafe {
            let dev = ID3D11Device::from_raw_borrowed(&device).ok_or("null capture device")?;
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut tex = None;
            dev.CreateTexture2D(&desc, None, Some(&mut tex))?;
            (tex.ok_or("CreateTexture2D returned nothing")?, dev.GetImmediateContext()?)
        };
        let cfg = Config { width: w, height: h, fps, bitrate_kbps: bitrate, preset, dither, tile: 0, ten_bit, hdr: false };
        println!("{cfg:?}; move {} frames at {speed} px/frame, then still {} frames", s.move_frames, s.still_frames);
        let mut enc = Encoder::new(device, cfg)?;
        let mut stream = fs::File::create(out.join("out.h265"))?;
        let mut csv = String::from("frame,phase,bytes,idr,encode_ms,avg_qp\n");
        for i in 0..s.move_frames + s.still_frames {
            let moving = i < s.move_frames;
            // 静止段和专业模式一样不给新画面，编码器重编上一帧
            let src = if moving || i == 0 {
                s.frame(&base, i, &mut frame);
                unsafe { ctx.UpdateSubresource(&tex, 0, None, frame.as_ptr() as _, w * 4, 0) };
                Some(tex.as_raw())
            } else {
                None
            };
            let t0 = Instant::now();
            let f = enc.encode(src, false)?;
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            stream.write_all(&f.data)?;
            let phase = if moving { "move" } else { "still" };
            csv += &format!("{i},{phase},{},{},{ms:.2},{}\n", f.data.len(), f.is_idr as u8, f.avg_qp);
        }
        fs::write(out.join("frames.csv"), csv)?;
        println!("done: {}", out.display());
        Ok(())
    }

    /// 只有颜色换算、不编码时的误差（相当于无损编码的上限）：按 shaders/rgb_to_yuv444.hlsl 的公式把 BGRA 换成 8bit YUV，
    /// 再按 BT.709 全范围换回 RGB。返回 (PSNR, 最大误差, 换出来的 YUV 4:4:4 平面排布)；YUV 用来单独核对解码那一段
    /// （用同一条 ffmpeg 命令换回 RGB，应当得到同样的下限）。
    fn conversion_floor(bgra: &[u8], w: usize, dither: bool) -> (f64, u8, Vec<u8>) {
        let n = bgra.len() / 4;
        let mut yuv = vec![0u8; n * 3];
        const BAYER8: [u8; 64] = [
            0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60, 28, 52, 20, 62, 30,
            54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15, 47, 7, 39, 13, 45, 5, 37, 63, 31, 55,
            23, 61, 29, 53, 21,
        ];
        let (mut max, mut sq) = (0u8, 0u64);
        for (p, px) in bgra.chunks_exact(4).enumerate() {
            let (x, y) = (p % w, p / w);
            let [b, g, r] = [px[0], px[1], px[2]].map(|c| c as f64 / 255.0);
            let t = if dither { (BAYER8[(y & 7) * 8 + (x & 7)] as f64 + 0.5) / 64.0 } else { 0.5 };
            let q = |v: f64| (v + t).floor().clamp(0.0, 255.0);
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let yy = q(luma * 255.0);
            let cb = q((b - luma) / 1.8556 * 255.0 + 128.0) - 128.0;
            let cr = q((r - luma) / 1.5748 * 255.0 + 128.0) - 128.0;
            yuv[p] = yy as u8;
            yuv[n + p] = (cb + 128.0) as u8;
            yuv[2 * n + p] = (cr + 128.0) as u8;
            let r2 = yy + 1.5748 * cr;
            let b2 = yy + 1.8556 * cb;
            let g2 = (yy - 0.2126 * r2 - 0.0722 * b2) / 0.7152;
            for (orig, back) in [px[2], px[1], px[0]].into_iter().zip([r2, g2, b2]) {
                let e = orig.abs_diff(back.round().clamp(0.0, 255.0) as u8);
                max = max.max(e);
                sq += (e as u64) * (e as u64);
            }
        }
        let mse = sq as f64 / (n * 3) as f64;
        (if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() }, max, yuv)
    }

    pub fn curve(args: &[String]) -> Res {
        use std::io::Read;
        let [dir] = args else {
            return Err("curve DIR  (decoded rgb24 frames on stdin)".into());
        };
        let dir = PathBuf::from(dir);
        let s = Synth::load(dir.join("synth.txt"))?;
        let (w, h) = (s.w as usize, s.h as usize);
        let sizes: Vec<usize> = fs::read_to_string(dir.join("frames.csv"))?
            .lines()
            .skip(1)
            .filter_map(|l| l.split(',').nth(2)?.parse().ok())
            .collect();
        let base = test_pattern(w, h);
        let (mut reff, mut dec) = (vec![0u8; w * h * 4], vec![0u8; w * h * 3]);
        let mut stdin = std::io::stdin().lock();
        // 每帧：PSNR、最大误差（三个通道里最大的）、平均误差
        let mut rows: Vec<(f64, u8, f64)> = Vec::new();
        let mut last_dec = Vec::new();
        while stdin.read_exact(&mut dec).is_ok() {
            last_dec.clone_from(&dec);
            s.frame(&base, rows.len() as u32, &mut reff);
            let (mut max, mut sum, mut sq) = (0u8, 0u64, 0u64);
            for p in 0..w * h {
                for c in 0..3 {
                    let e = reff[p * 4 + 2 - c].abs_diff(dec[p * 3 + c]);
                    max = max.max(e);
                    sum += e as u64;
                    sq += (e as u64) * (e as u64);
                }
            }
            let n = (w * h * 3) as f64;
            let mse = sq as f64 / n;
            let psnr = if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() };
            rows.push((psnr, max, sum as f64 / n));
        }
        if rows.len() <= s.move_frames as usize {
            return Err(format!("only {} decoded frames, the still phase starts at {}", rows.len(), s.move_frames).into());
        }
        let mut csv = String::from("frame,phase,bytes,psnr,max_err,mean_err\n");
        for (i, (psnr, max, mean)) in rows.iter().enumerate() {
            let phase = if i < s.move_frames as usize { "move" } else { "still" };
            csv += &format!("{i},{phase},{},{psnr:.2},{max},{mean:.4}\n", sizes.get(i).copied().unwrap_or(0));
        }
        fs::write(dir.join("curve.csv"), csv)?;

        let still = &rows[s.move_frames as usize..];
        let ms = |k: usize| k as f64 * 1000.0 / s.fps as f64;
        let first = |ok: &dyn Fn(&(f64, u8, f64)) -> bool| {
            still.iter().position(ok).map_or("never".to_owned(), |k| format!("frame +{k} ({:.0} ms)", ms(k)))
        };
        let regress = still.windows(2).filter(|p| p[1].0 < p[0].0 - 0.5).count();
        let still_sizes = &sizes[(s.move_frames as usize).min(sizes.len())..];
        let budget = s.bitrate as usize * 1000 / 8 / s.fps.max(1) as usize;
        let last = still[still.len() - 1];
        s.frame(&base, s.move_frames, &mut reff);
        let floor = conversion_floor(&reff, w, s.dither);
        fs::write(dir.join("still_ref.bgra"), &reff)?;
        fs::write(dir.join("still_floor.yuv"), &floor.2)?;
        let report = format!(
            "dither {}; conversion floor of the still picture (no coding loss): PSNR {:.2} dB, max err {}\n\
             still phase: {} frames ({:.1} s)\n\
             at stop (still +0): PSNR {:.2} dB, max err {}, mean err {:.3}\n\
             reach the floor (max err <= {}, PSNR >= {:.2} dB): {}\n\
             final (still +{}): PSNR {:.2} dB, max err {}, mean err {:.4}\n\
             regressions (PSNR drop > 0.5 dB between consecutive still frames): {regress}\n\
             largest still frame: {} bytes (one-frame budget at {} kbps / {} fps: {budget} bytes); \
             still phase total {} bytes\n",
            if s.dither { "on" } else { "off" },
            floor.0,
            floor.1,
            still.len(),
            ms(still.len()) / 1000.0,
            still[0].0,
            still[0].1,
            still[0].2,
            floor.1,
            floor.0 - 0.5,
            first(&|r| r.1 <= floor.1 && r.0 >= floor.0 - 0.5),
            still.len() - 1,
            last.0,
            last.1,
            last.2,
            still_sizes.iter().max().copied().unwrap_or(0),
            s.bitrate,
            s.fps,
            still_sizes.iter().sum::<usize>(),
        );
        // 最后一帧按测试图的四段分别算，看剩下的误差集中在哪类内容上
        let band = (h / 4).max(1);
        let names = ["color chart + grid", "gradients", "1px lines + red/blue checkerboard", "glyphs"];
        let report = (0..4).fold(report, |acc, b| {
            let (mut max, mut sq, mut n) = (0u8, 0u64, 0u64);
            for p in b * band * w..((b + 1) * band).min(h) * w {
                for c in 0..3 {
                    let e = reff[p * 4 + 2 - c].abs_diff(last_dec[p * 3 + c]);
                    max = max.max(e);
                    sq += (e as u64) * (e as u64);
                    n += 1;
                }
            }
            let mse = sq as f64 / n.max(1) as f64;
            let psnr = if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() };
            acc + &format!("  final, band {}: PSNR {psnr:.2} dB, max err {max}\n", names[b])
        });
        print!("{report}");
        fs::write(dir.join("summary.txt"), report)?;
        Ok(())
    }

    /// HDR 测试图（scRGB，每像素 RGBA 四个 f32，已经过 FP16 往返，和显卡拿到的一样）：上半是 CG 测试图（sRGB → 线性 × SDR 白），
    /// 下半前一段是 0–1000 尼特的灰阶，后一段是一排高光色块。
    fn hdr_pattern(w: usize, h: usize, sdr_white_nits: f32) -> Vec<[f32; 4]> {
        let base = test_pattern(w, h);
        let srgb = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        // scRGB 1.0 = 80 尼特
        let patches: [[f32; 3]; 8] = [
            [400.0, 0.0, 0.0],
            [0.0, 400.0, 0.0],
            [0.0, 0.0, 400.0],
            [600.0, 600.0, 600.0],
            [1000.0, 1000.0, 1000.0],
            // BT.709 装不下、BT.2020 装得下的绿（scRGB 用负的红表示）
            [-40.0, 300.0, 0.0],
            // 超过 PQ 上限（10000 尼特），截成满
            [12000.0, 12000.0, 12000.0],
            [0.0, 0.0, 0.0],
        ];
        let mut out = vec![[0f32; 4]; w * h];
        for y in 0..h {
            for x in 0..w {
                let rgb = if y < h / 2 {
                    let p = (y * w + x) * 4;
                    let k = sdr_white_nits / 80.0;
                    [srgb(base[p + 2]) * k, srgb(base[p + 1]) * k, srgb(base[p]) * k]
                } else if y < h * 3 / 4 {
                    let v = 1000.0 * x as f32 / (w - 1) as f32 / 80.0;
                    [v, v, v]
                } else {
                    let c = patches[x * patches.len() / w];
                    [c[0] / 80.0, c[1] / 80.0, c[2] / 80.0]
                };
                out[y * w + x] = [f16_round(rgb[0]), f16_round(rgb[1]), f16_round(rgb[2]), 1.0];
            }
        }
        out
    }

    /// 按 shaders/scrgb_to_pq444_10.hlsl 的公式算一个像素的 10 位 Y / U / V
    fn pq_yuv(px: [f32; 4]) -> [u16; 3] {
        const M: [[f32; 3]; 3] = [[0.6274040, 0.3292820, 0.0433136], [0.0690970, 0.9195400, 0.0113612], [0.0163916, 0.0880132, 0.8955950]];
        let lin = [px[0] * 80.0, px[1] * 80.0, px[2] * 80.0];
        let pq = |n: f32| {
            let y = (n.max(0.0) / 10000.0).clamp(0.0, 1.0).powf(0.1593017578125);
            ((0.8359375 + 18.8515625 * y) / (1.0 + 18.6875 * y)).powf(78.84375)
        };
        let rgb: Vec<f32> = M.iter().map(|r| pq(r[0] * lin[0] + r[1] * lin[1] + r[2] * lin[2])).collect();
        let y = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
        let cb = (rgb[2] - y) / 1.8814;
        let cr = (rgb[0] - y) / 1.4746;
        let q = |v: f32| (v + 0.5).floor().clamp(0.0, 1023.0) as u16;
        [q(y * 1023.0), q(cb * 1023.0 + 512.0), q(cr * 1023.0 + 512.0)]
    }

    /// f32 → 半精度的位（就近舍入）；只覆盖测试图用到的范围（正常数、零、负数）
    fn f16_bits(v: f32) -> u16 {
        let x = v.to_bits();
        let sign = ((x >> 16) & 0x8000) as u16;
        let exp = ((x >> 23) & 0xff) as i32 - 127 + 15;
        let mant = x & 0x7f_ffff;
        if exp <= 0 {
            if exp < -10 {
                return sign;
            }
            let m = (mant | 0x80_0000) >> (14 - exp);
            return sign | ((m + 1) >> 1) as u16;
        }
        if exp >= 31 {
            return sign | 0x7c00;
        }
        let h = ((exp as u32) << 10 | mant >> 13) + ((mant >> 12) & 1);
        sign | h as u16
    }

    fn f16_value(h: u16) -> f32 {
        let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exp = ((h >> 10) & 0x1f) as i32;
        let mant = (h & 0x3ff) as f32;
        sign * match exp {
            0 => mant * 2f32.powi(-24),
            31 => f32::INFINITY,
            e => (1.0 + mant / 1024.0) * 2f32.powi(e - 15),
        }
    }

    fn f16_round(v: f32) -> f32 {
        f16_value(f16_bits(v))
    }

    pub fn hdr(args: &[String]) -> Res {
        use windows::{
            core::Interface,
            Win32::Graphics::{Direct3D11::*, Dxgi::Common::*},
        };
        let (mut out, mut seconds, mut sdr_white, mut bitrate, mut preset) = (PathBuf::new(), 2u32, 240f32, 30_000u32, 4u8);
        let mut it = args.iter();
        while let Some(k) = it.next() {
            let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
            match k.as_str() {
                "--out" => out = PathBuf::from(v),
                "--seconds" => seconds = v.parse()?,
                "--sdr-white" => sdr_white = v.parse()?,
                "--bitrate" => bitrate = v.parse()?,
                "--preset" => preset = v.parse()?,
                _ => return Err(format!("unknown option {k}").into()),
            }
        }
        if out.as_os_str().is_empty() {
            return Err("--out DIR is required".into());
        }
        fs::create_dir_all(&out)?;
        let (cap, w, h) = nvidia_capturer()?;
        let (wu, hu) = (w as usize, h as usize);
        let px = hdr_pattern(wu, hu, sdr_white);
        let mut expected = vec![0u8; wu * hu * 6];
        for (i, &p) in px.iter().enumerate() {
            for (plane, v) in pq_yuv(p).into_iter().enumerate() {
                let o = (plane * wu * hu + i) * 2;
                expected[o..o + 2].copy_from_slice(&v.to_le_bytes());
            }
        }
        fs::write(out.join("expected.yuv"), &expected)?;
        fs::write(out.join("hdr.txt"), format!("{w} {h} {sdr_white}\n"))?;
        let halves: Vec<u16> = px.iter().flat_map(|p| p.iter().map(|&c| f16_bits(c))).collect();

        let device = cap.device().device;
        let (tex, ctx) = unsafe {
            let dev = ID3D11Device::from_raw_borrowed(&device).ok_or("null capture device")?;
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut tex = None;
            dev.CreateTexture2D(&desc, None, Some(&mut tex))?;
            (tex.ok_or("CreateTexture2D returned nothing")?, dev.GetImmediateContext()?)
        };
        let cfg = Config { width: w, height: h, fps: 60, bitrate_kbps: bitrate, preset, dither: false, tile: 0, ten_bit: true, hdr: true };
        println!("{cfg:?}; SDR white {sdr_white} nits, still {seconds} s");
        let mut enc = Encoder::new(device, cfg)?;
        let mut stream = fs::File::create(out.join("out.h265"))?;
        unsafe { ctx.UpdateSubresource(&tex, 0, None, halves.as_ptr() as _, w * 8, 0) };
        let mut ms = Vec::new();
        for i in 0..seconds * 60 {
            let t0 = Instant::now();
            let f = enc.encode((i == 0).then(|| tex.as_raw()), false)?;
            ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            stream.write_all(&f.data)?;
        }
        ms.sort_by(|a, b| a.total_cmp(b));
        println!("done: {} ({} frames, encode p50 {:.2} ms)", out.display(), ms.len(), ms[ms.len() / 2]);
        Ok(())
    }

    /// 主控端着色器（gl.rs 的 HDR 分支）对一个 10 位 Y / U / V 的结果（8bit RGB）
    fn client_hdr_rgb(yuv: [u16; 3], sdr_white: f64) -> [u8; 3] {
        const M: [[f64; 3]; 3] = [[1.660491, -0.587641, -0.072850], [-0.124550, 1.132900, -0.008349], [-0.018151, -0.100579, 1.118730]];
        let y = yuv[0] as f64 / 1023.0;
        let cb = (yuv[1] as f64 - 512.0) / 1023.0;
        let cr = (yuv[2] as f64 - 512.0) / 1023.0;
        let r2 = y + 1.4746 * cr;
        let b2 = y + 1.8814 * cb;
        let e = [r2, (y - 0.2627 * r2 - 0.0593 * b2) / 0.6780, b2].map(|v| v.clamp(0.0, 1.0));
        let nits = e.map(|v| {
            let q = v.powf(1.0 / 78.84375);
            10000.0 * ((q - 0.8359375).max(0.0) / (18.8515625 - 18.6875 * q)).powf(1.0 / 0.1593017578125)
        });
        let rel = nits.map(|n| n / sdr_white);
        let mut lin = M.map(|r| (r[0] * rel[0] + r[1] * rel[1] + r[2] * rel[2]).max(0.0));
        let m = lin.iter().cloned().fold(0.0, f64::max);
        if m > 1.0 {
            lin = lin.map(|v| v / m);
        }
        lin.map(|v| {
            let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            (s.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
        })
    }

    pub fn hdr_rgb(args: &[String]) -> Res {
        let usage = "usage: hdrrgb DIR SHADER.rgb [DECODED.yuv]";
        let dir = PathBuf::from(args.first().ok_or(usage)?);
        let shader = fs::read(args.get(1).ok_or(usage)?)?;
        let meta = fs::read_to_string(dir.join("hdr.txt"))?;
        let v: Vec<f64> = meta.split_whitespace().filter_map(|s| s.parse().ok()).collect();
        let (w, h, white) = (v[0] as usize, v[1] as usize, v.get(2).copied().unwrap_or(240.0));
        // 给了解码出的同一帧（yuv444p10le，HEVC 解码逐位确定，软件解的和瘦客户机硬解的一样）就按它算公式：只剩着色器本身的误差；
        // 不给就按编码前的期望值算：再加上编码损失
        let expected = match args.get(2) {
            Some(p) => fs::read(p)?,
            None => fs::read(dir.join("expected.yuv"))?,
        };
        if shader.len() != w * h * 3 {
            return Err(format!("{} bytes of RGB, expected {}x{}x3", shader.len(), w, h).into());
        }
        let sample = |plane: usize, i: usize| {
            let o = (plane * w * h + i) * 2;
            u16::from_le_bytes([expected[o], expected[o + 1]])
        };
        let original = test_pattern(w, h);
        let regions: [(&str, std::ops::Range<usize>); 3] =
            [("CG at SDR white", 0..h / 2), ("0-1000 nit ramp", h / 2..h * 3 / 4), ("highlight patches", h * 3 / 4..h)];
        let mut report = format!("{w}x{h}, SDR white {white} nits\n");
        let stat = |rows: std::ops::Range<usize>, want: &dyn Fn(usize) -> [u8; 3]| {
            let (mut max, mut sum, mut diff, mut n) = (0u8, 0u64, 0u64, 0u64);
            for y in rows {
                for x in 0..w {
                    let i = y * w + x;
                    let e = want(i);
                    let d = (0..3).map(|c| shader[i * 3 + c].abs_diff(e[c])).max().unwrap_or(0);
                    max = max.max(d);
                    sum += d as u64;
                    diff += (d > 1) as u64;
                    n += 1;
                }
            }
            format!("max {max}, mean {:.3}, off by more than 1: {:.3}%", sum as f64 / n as f64, diff as f64 * 100.0 / n as f64)
        };
        for (name, rows) in &regions {
            let want = |i: usize| client_hdr_rgb([sample(0, i), sample(1, i), sample(2, i)], white);
            report += &format!("shader vs formula, {name:18}: {}\n", stat(rows.clone(), &want));
        }
        let orig = |i: usize| [original[i * 4 + 2], original[i * 4 + 1], original[i * 4]];
        report += &format!("shader vs the 8-bit original, CG at SDR white: {}\n", stat(0..h / 2, &orig));
        // 转换链本身的下限（不编码）：编码前的期望值按公式还原，和原图比
        let pre = fs::read(dir.join("expected.yuv"))?;
        let pre_sample = |plane: usize, i: usize| {
            let o = (plane * w * h + i) * 2;
            u16::from_le_bytes([pre[o], pre[o + 1]])
        };
        let (mut max, mut sum, mut over, mut n) = (0u8, 0u64, 0u64, 0u64);
        for i in 0..w * h / 2 {
            let got = client_hdr_rgb([pre_sample(0, i), pre_sample(1, i), pre_sample(2, i)], white);
            let d = (0..3).map(|c| got[c].abs_diff(orig(i)[c])).max().unwrap_or(0);
            max = max.max(d);
            sum += d as u64;
            over += (d > 1) as u64;
            n += 1;
        }
        report += &format!(
            "no encoding (formula on the pre-encode values) vs the 8-bit original, CG at SDR white: max {max}, mean {:.3}, off by more than 1: {:.3}%\n",
            sum as f64 / n as f64,
            over as f64 * 100.0 / n as f64
        );
        print!("{report}");
        fs::write(dir.join("hdrrgb.txt"), report)?;
        Ok(())
    }

    pub fn hdr_check(args: &[String]) -> Res {
        use std::io::Read;
        let dir = PathBuf::from(args.first().ok_or("usage: hdrcheck DIR < decoded yuv444p10le")?);
        let wh = fs::read_to_string(dir.join("hdr.txt"))?;
        let mut it = wh.split_whitespace().map(|s| s.parse::<usize>());
        let (w, h) = (it.next().ok_or("bad hdr.txt")??, it.next().ok_or("bad hdr.txt")??);
        let expected = fs::read(dir.join("expected.yuv"))?;
        let frame_bytes = w * h * 6;
        let mut stdin = std::io::stdin().lock();
        let mut buf = vec![0u8; frame_bytes];
        let (mut first, mut last, mut n) = (None, None, 0);
        while stdin.read_exact(&mut buf).is_ok() {
            if first.is_none() {
                first = Some(buf.clone());
            }
            last = Some(buf.clone());
            n += 1;
        }
        let sample = |b: &[u8], i: usize| u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]) as i32;
        let regions: [(&str, std::ops::Range<usize>); 3] =
            [("CG at SDR white", 0..h / 2), ("0-1000 nit ramp", h / 2..h * 3 / 4), ("highlight patches", h * 3 / 4..h)];
        let mut report = format!("{n} frames decoded, {w}x{h}\n");
        for (name, frame) in [("first", first), ("last", last)] {
            let Some(f) = frame else { continue };
            for (rname, rows) in &regions {
                let mut line = format!("{name:5} {rname:18}:");
                for (pi, pname) in ["Y", "U", "V"].iter().enumerate() {
                    let (mut max, mut sum, mut sq, mut cnt) = (0i32, 0f64, 0f64, 0f64);
                    for y in rows.clone() {
                        for x in 0..w {
                            let i = pi * w * h + y * w + x;
                            let d = (sample(&f, i) - sample(&expected, i)).abs();
                            max = max.max(d);
                            sum += d as f64;
                            sq += (d * d) as f64;
                            cnt += 1.0;
                        }
                    }
                    let psnr = if sq == 0.0 { f64::INFINITY } else { 10.0 * (1023.0f64.powi(2) / (sq / cnt)).log10() };
                    line += &format!("  {pname} max {max} mean {:.3} PSNR {psnr:.2}", sum / cnt);
                }
                report += &line;
                report += "\n";
            }
        }
        print!("{report}");
        fs::write(dir.join("hdrcheck.txt"), report)?;
        Ok(())
    }
}
