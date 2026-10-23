//! 第 5 步验证工具（瘦客户机上跑，不需要桌面）：显卡硬解 + 视频处理单元转 RGB，和被控端原图比颜色、量每帧耗时、
//! 试导出显存画面。
//!
//!   vaapi_check IN.h265 [--ref REF.bgra --ref-frame 30] [--out DIR] [--gl] [--tiled]
//!
//! `--gl`：用着色器转 RGB（专业窗口 1:1 时走的路径），参考帧上再用视频处理单元转一遍逐像素比，读回的着色器结果存
//! DIR/shader<帧号>.rgb（可拿去和软件解码按公式算的结果逐像素核对）。
//! `--hdr N`（和 `--gl` 一起）：HDR 码流，着色器按 BT.2020 + PQ 还原、普通内容的白是 N 尼特（P1 第 4 项；被控端验证工具
//! `capture_encode hdr` 出的码流，读回的结果用 `capture_encode hdrrgb` 核对）。
//! `--hdr-pass`（和 `--gl` 一起）：HDR 原样输出（本机屏开着 HDR 时专业窗口走的路径）——着色器只还原 PQ 编码的 BT.2020 R'G'B'，
//! 画进 GBM 分块的 10 位画面；参考帧读回存 DIR/shader10_<帧号>.rgb（每像素 R G B 三个 16 位小端数，0–1023），
//! 拿去和软件解码同一帧按公式算的结果逐像素核对。
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
    use ddpro_dec::{Converter, Decoder, Device, GlConverter, RgbStandard};
    use ddpro_media::hevc;
    use std::{fs, path::PathBuf, time::Instant};

    /// 轮流使用的输出画面数（显示端拿着一张时写另一张）
    const SURFACES: usize = 4;

    struct Opts {
        input: String,
        reference: Option<String>,
        ref_frame: usize,
        out: Option<PathBuf>,
        /// 用着色器转 RGB（专业窗口 1:1 时的路径）；参考帧上再用视频处理单元转一遍，逐像素比两者
        gl: bool,
        /// 输出画面用 Intel 的 Y / X 分块排布（专业窗口在合成器能用时就这样建）
        tiled: bool,
        /// HDR 码流（P1 第 4 项）：着色器按 BT.2020 + PQ 还原，值是被控端普通内容的白（尼特）；要和 `--gl` 一起用
        hdr: Option<u32>,
        /// HDR 原样输出：着色器画 10 位 PQ 编码的 BT.2020 R'G'B'；要和 `--gl` 一起用
        hdr_pass: bool,
    }

    fn opts() -> Result<Opts, Box<dyn std::error::Error>> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let input = args
            .first()
            .ok_or("usage: vaapi_check IN.h265 [--ref REF.bgra --ref-frame N] [--out DIR] [--gl]")?
            .clone();
        let mut o = Opts {
            input,
            reference: None,
            ref_frame: 30,
            out: None,
            gl: false,
            tiled: false,
            hdr: None,
            hdr_pass: false,
        };
        let mut it = args[1..].iter();
        while let Some(k) = it.next() {
            match k.as_str() {
                "--gl" => {
                    o.gl = true;
                    continue;
                }
                "--tiled" => {
                    o.tiled = true;
                    continue;
                }
                "--hdr-pass" => {
                    o.hdr_pass = true;
                    continue;
                }
                _ => {}
            }
            let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
            match k.as_str() {
                "--ref" => o.reference = Some(v.clone()),
                "--ref-frame" => o.ref_frame = v.parse()?,
                "--out" => o.out = Some(PathBuf::from(v)),
                "--hdr" => o.hdr = Some(v.parse()?),
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
        let mut gl: Option<GlConverter> = None;
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
                // 着色器也认 10bit（Y410）：专业协议在 HDR 时谈 10bit（P1 第 4 项）
                if !f.chroma_444 || !(f.bit_depth == 8 || (o.gl && f.bit_depth == 10)) {
                    return Err("the decoded surface is not 4:4:4 8bit".into());
                }
                // --tiled：和专业窗口一样，着色器用 GBM 按 Intel Y / X 分块分配输出画面，视频处理单元导入同一组
                let (c, g) = if o.hdr_pass {
                    // 和专业窗口 HDR 原样输出一样：GBM 分块的 10 位画面，只有着色器画
                    let g = GlConverter::new_hdr(w, h, SURFACES, &[0x0100_0000_0000_0002, 0x0100_0000_0000_0001])?;
                    println!("shader conversion: HDR as is (10-bit BT.2020 PQ), output modifier {:#x}", g.export(0)?.modifiers[0]);
                    (Converter::new(&dev, w, h, SURFACES)?, Some(g))
                } else if o.tiled {
                    let g = GlConverter::new(w, h, SURFACES, &[0x0100_0000_0000_0002, 0x0100_0000_0000_0001])?;
                    let images = (0..SURFACES).map(|i| g.export(i)).collect::<Result<Vec<_>, _>>()?;
                    (Converter::with_images(&dev, w, h, &images)?, Some(g))
                } else {
                    let c = Converter::new(&dev, w, h, SURFACES)?;
                    let g = if o.gl { Some(GlConverter::on(&c)?) } else { None };
                    (c, g)
                };
                if let Some(g) = &g {
                    println!("shader conversion on {}", g.renderer());
                }
                let mut g = g;
                if let (Some(g), Some(white)) = (g.as_mut(), o.hdr) {
                    g.set_hdr(Some(white))?;
                    println!("shader conversion: HDR (BT.2020 + PQ), SDR white {white} nits");
                }
                // 只给了 --tiled 没给 --gl：输出画面照样是 GBM 分块的，但只用视频处理单元转
                gl = if o.gl { g } else { None };
                conv = Some(c);
            }
            let Some(conv) = conv.as_mut() else { unreachable!() };
            let slot = decoded % SURFACES;
            match gl.as_mut() {
                Some(g) => g.convert(&dec, &frame, slot)?,
                None => {
                    conv.convert(&frame, slot, RgbStandard::Bt709)?;
                    conv.wait(slot)?;
                }
            }
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
            if let (Some(g), true, true) = (gl.as_mut(), i == o.ref_frame, o.hdr_pass) {
                let rgb = g.read_rgb10(slot)?;
                let peak = rgb.iter().copied().max().unwrap_or(0);
                println!("frame {i}: read back {} 10-bit values, max {peak}", rgb.len());
                if let Some(dir) = &o.out {
                    fs::create_dir_all(dir)?;
                    let bytes: Vec<u8> = rgb.iter().flat_map(|v| v.to_le_bytes()).collect();
                    fs::write(dir.join(format!("shader10_{i}.rgb")), bytes)?;
                }
            } else if let (Some(g), true) = (gl.as_mut(), i == o.ref_frame) {
                // 每张输出画面都画同一帧（着色器写一次、视频处理单元写一次），都从着色器那侧读回，数最上面 32 行里的纯黑像素。
                // 不用视频处理单元那侧读回：iHD 读导入的 GBM 画面（vaGetImage）不可靠——读出过整块黑、视频处理单元写过之后再读
                // 会在驱动里崩溃（2026-10-21 瘦客户机实测）；正式显示不读回，不受影响
                let black = |rgb: &[u8]| rgb[..w as usize * 32 * 3].chunks_exact(3).filter(|p| p == &[0, 0, 0]).count();
                for round in 0..3 {
                    for s in 0..SURFACES {
                        g.convert(&dec, &frame, s)?;
                        let by_shader = black(&g.read_rgb(s)?);
                        conv.convert(&frame, s, RgbStandard::Bt709)?;
                        conv.wait(s)?;
                        let by_vpp = black(&g.read_rgb(s)?);
                        println!("round {round} slot {s}: black in the top 32 rows — written by shader {by_shader}, by video processor {by_vpp}");
                    }
                }
                // 同一帧两种转法各写一张输出画面，逐像素比（视频处理单元自己有约 0.5% 的像素差 1 级）
                let other = (slot + 1) % SURFACES;
                g.convert(&dec, &frame, slot)?;
                conv.convert(&frame, other, RgbStandard::Bt709)?;
                conv.wait(other)?;
                let (by_gl, by_vpp) = (g.read_rgb(slot)?, g.read_rgb(other)?);
                let (mut diff, mut max) = (0usize, 0u8);
                for (a, b) in by_gl.chunks_exact(3).zip(by_vpp.chunks_exact(3)) {
                    let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
                    diff += (d > 0) as usize;
                    max = max.max(d);
                }
                let n = by_gl.len() / 3;
                println!("frame {i} shader vs video processor: {diff} of {n} pixels differ ({:.3}%), max {max}", diff as f64 * 100.0 / n as f64);
                if let Some(r) = &reference {
                    println!("frame {i} vs original, shader: {}", compare(r, &by_gl, w, h)?);
                    println!("frame {i} vs original, video processor: {}", compare(r, &by_vpp, w, h)?);
                }
                if let Some(dir) = &o.out {
                    fs::create_dir_all(dir)?;
                    fs::write(dir.join(format!("shader{i}.rgb")), &by_gl)?;
                }
            } else if let (Some(r), true) = (&reference, i == o.ref_frame) {
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
