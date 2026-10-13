//! 第 5 步验证工具（瘦客户机上跑，不需要桌面）：显卡硬解 + 视频处理单元转 RGB，和被控端原图比颜色、量每帧耗时、
//! 试导出显存画面。
//!
//!   vaapi_check IN.h265 [--ref REF.bgra --ref-frame 30] [--out DIR]
//!
//! - 开显卡，报驱动和 HEVC 4:4:4 / 视频处理能力；
//! - 逐帧硬解（等显卡解完）→ 转 RGB（等显卡转完），记两段耗时；
//! - 第一帧报实际的表面格式：不是 4:4:4 8bit 就失败；
//! - 第 ref-frame 帧读回 RGB，两种 RGB 颜色标准各转一次，和原图（第 3 步验证工具在被控端存的 BGRA）逐像素比；
//! - 导出一张 RGB 画面为 dmabuf，报格式和 modifier（第 5 步窗口要用）。

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("vaapi_check runs on Linux only");
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
    use std::{fs, path::PathBuf, time::Instant};

    /// 轮流使用的输出画面数（显示端拿着一张时写另一张）
    const SURFACES: usize = 4;

    struct Opts {
        input: String,
        reference: Option<String>,
        ref_frame: usize,
        out: Option<PathBuf>,
    }

    fn opts() -> Result<Opts, Box<dyn std::error::Error>> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let input = args
            .first()
            .ok_or("usage: vaapi_check IN.h265 [--ref REF.bgra --ref-frame N] [--out DIR]")?
            .clone();
        let mut o = Opts {
            input,
            reference: None,
            ref_frame: 30,
            out: None,
        };
        let mut it = args[1..].iter();
        while let Some(k) = it.next() {
            let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
            match k.as_str() {
                "--ref" => o.reference = Some(v.clone()),
                "--ref-frame" => o.ref_frame = v.parse()?,
                "--out" => o.out = Some(PathBuf::from(v)),
                _ => return Err(format!("unknown option {k}").into()),
            }
        }
        Ok(o)
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let o = opts()?;
        let aus = hevc::split_access_units(&fs::read(&o.input)?);
        println!("{} access units from {}", aus.len(), o.input);
        let reference = o.reference.as_ref().map(fs::read).transpose()?;

        let dev = Device::open()?;
        println!("driver: {}", dev.vendor()?);
        let caps = dev.probe()?;
        println!("caps: {caps:?}");
        if !caps.hevc_444_8bit || !caps.video_proc {
            return Err("this GPU cannot decode HEVC 4:4:4 8bit or has no video processing".into());
        }

        let mut dec = Decoder::new(&dev)?;
        let mut conv: Option<Converter> = None;
        let (mut decode_ms, mut convert_ms) = (Vec::new(), Vec::new());
        let (mut decoded, mut errors) = (0usize, 0usize);
        for (i, au) in aus.iter().enumerate() {
            let t0 = Instant::now();
            let frame = match dec.decode(au) {
                Ok(Some(f)) => f,
                Ok(None) => continue,
                Err(e) => {
                    errors += 1;
                    eprintln!("frame {i}: {e}");
                    continue;
                }
            };
            dec.wait(&frame)?;
            let t1 = Instant::now();
            let (w, h) = (frame.width(), frame.height());
            if conv.is_none() {
                let f = frame.format();
                println!("first frame: {w}x{h}, surface format {} (4:4:4: {}, {} bit)", f.name, f.chroma_444, f.bit_depth);
                if !f.chroma_444 || f.bit_depth != 8 {
                    return Err("the decoded surface is not 4:4:4 8bit".into());
                }
                conv = Some(Converter::new(&dev, w, h, SURFACES)?);
            }
            let Some(conv) = conv.as_mut() else { unreachable!() };
            let slot = decoded % SURFACES;
            conv.convert(&frame, slot, RgbStandard::Bt709)?;
            conv.wait(slot)?;
            let t2 = Instant::now();
            decode_ms.push((t1 - t0).as_secs_f64() * 1000.0);
            convert_ms.push((t2 - t1).as_secs_f64() * 1000.0);

            if decoded == 0 {
                let e = conv.export(slot)?;
                println!(
                    "export: fourcc {} {}x{}, modifiers {:x?}, {} fd(s), layers {:?}",
                    fourcc(e.fourcc),
                    e.width,
                    e.height,
                    e.modifiers,
                    e.fds.len(),
                    e.layers
                );
            }
            if let (Some(r), true) = (&reference, i == o.ref_frame) {
                for std in [RgbStandard::Bt709, RgbStandard::Srgb] {
                    conv.convert(&frame, slot, std)?;
                    conv.wait(slot)?;
                    let rgb = conv.read_rgb(slot)?;
                    println!("frame {i} vs original, RGB standard {std:?}: {}", compare(r, &rgb, w, h)?);
                    if let Some(dir) = &o.out {
                        fs::create_dir_all(dir)?;
                        fs::write(dir.join(format!("vaapi{i}_{std:?}.rgb")), &rgb)?;
                    }
                }
            }
            decoded += 1;
        }
        println!("decoded {decoded} of {} frames, {errors} decode error(s)", aus.len());
        println!("decode (until the GPU is done) ms: {}", stats(&mut decode_ms));
        println!("convert to RGB (until the GPU is done) ms: {}", stats(&mut convert_ms));
        if errors > 0 || decoded == 0 {
            return Err("decode errors".into());
        }
        Ok(())
    }

    fn fourcc(v: u32) -> String {
        v.to_le_bytes().iter().map(|&b| b as char).collect()
    }

    fn stats(v: &mut [f64]) -> String {
        if v.is_empty() {
            return "-".into();
        }
        v.sort_by(|a, b| a.total_cmp(b));
        let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        let avg = v.iter().sum::<f64>() / v.len() as f64;
        format!("avg {avg:.2} p50 {:.2} p95 {:.2} max {:.2}", p(0.5), p(0.95), p(1.0))
    }

    /// 原图 BGRA 和读回的 RGB 逐像素比：每通道最大误差、平均误差、PSNR（和第 3 步的 compare 同一算法）。
    fn compare(reference: &[u8], rgb: &[u8], w: u32, h: u32) -> Result<String, Box<dyn std::error::Error>> {
        let n = (w * h) as usize;
        if reference.len() != n * 4 || rgb.len() != n * 3 {
            return Err(format!("size mismatch: reference {} bytes, decoded {} bytes for {w}x{h}", reference.len(), rgb.len()).into());
        }
        let (mut max, mut sum, mut sq) = ([0u8; 3], [0u64; 3], 0f64);
        for (r, d) in reference.chunks_exact(4).zip(rgb.chunks_exact(3)) {
            let want = [r[2], r[1], r[0]];
            for c in 0..3 {
                let e = want[c].abs_diff(d[c]);
                max[c] = max[c].max(e);
                sum[c] += e as u64;
                sq += (e as f64) * (e as f64);
            }
        }
        let mse = sq / (n * 3) as f64;
        let psnr = if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0 * 255.0 / mse).log10() };
        let mean = |c: usize| sum[c] as f64 / n as f64;
        Ok(format!(
            "max abs error R/G/B {}/{}/{}, mean abs error R/G/B {:.3}/{:.3}/{:.3}, PSNR {psnr:.2} dB",
            max[0],
            max[1],
            max[2],
            mean(0),
            mean(1),
            mean(2)
        ))
    }
}
