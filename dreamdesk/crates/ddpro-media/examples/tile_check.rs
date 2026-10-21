//! 无损方块编解码验证：整屏 BGRA 原图按方块压缩再解开，逐字节和原图比，报压缩后大小和单线程耗时。
//!
//!   tile_check WIDTH HEIGHT A.bgra [B.bgra ...]
use ddpro_media::tile::{decode, encode, Grid};
use std::{fs, time::Instant};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: tile_check WIDTH HEIGHT A.bgra [B.bgra ...]";
    let w: u32 = args.first().ok_or(usage)?.parse()?;
    let h: u32 = args.get(1).ok_or(usage)?.parse()?;
    let grid = Grid::new(w, h);
    let mut bad_total = 0;
    for path in &args[2..] {
        let img = fs::read(path)?;
        if img.len() != (w * h * 4) as usize {
            return Err(format!("{path}: {} bytes, expected {}", img.len(), w * h * 4).into());
        }
        let tiles: Vec<Vec<u8>> = (0..grid.count())
            .map(|t| {
                let (x, y, tw, th) = grid.rect(t);
                let mut px = Vec::with_capacity((tw * th * 4) as usize);
                for row in y..y + th {
                    let start = ((row * w + x) * 4) as usize;
                    px.extend_from_slice(&img[start..start + (tw * 4) as usize]);
                }
                px
            })
            .collect();
        let t0 = Instant::now();
        let mut encoded = Vec::with_capacity(tiles.len());
        for (t, px) in tiles.iter().enumerate() {
            let (_, _, tw, th) = grid.rect(t);
            let mut out = Vec::new();
            encode(px, tw, th, &mut out)?;
            encoded.push(out);
        }
        let enc_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = Instant::now();
        let mut bad = 0;
        for (t, data) in encoded.iter().enumerate() {
            let (_, _, tw, th) = grid.rect(t);
            match decode(data) {
                Ok((dw, dh, rgba)) => {
                    let same = (dw, dh) == (tw, th)
                        && rgba
                            .chunks_exact(4)
                            .zip(tiles[t].chunks_exact(4))
                            .all(|(d, s)| d[0] == s[2] && d[1] == s[1] && d[2] == s[0] && d[3] == 255);
                    bad += !same as usize;
                }
                Err(e) => {
                    eprintln!("{path}: tile {t}: {e}");
                    bad += 1;
                }
            }
        }
        let dec_ms = t1.elapsed().as_secs_f64() * 1000.0;
        let bytes: usize = encoded.iter().map(Vec::len).sum();
        println!(
            "{path}: {} tiles, {:.2} MB ({:.2} bits/pixel), encode {enc_ms:.1} ms, decode {dec_ms:.1} ms (single thread), {bad} tile(s) differ",
            grid.count(),
            bytes as f64 / 1e6,
            bytes as f64 * 8.0 / (w * h) as f64
        );
        bad_total += bad;
    }
    if bad_total > 0 {
        return Err(format!("{bad_total} tile(s) did not round-trip").into());
    }
    Ok(())
}
