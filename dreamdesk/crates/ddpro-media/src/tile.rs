//! 静止后逐像素无损（P1 第 7 项第 2 步）：屏幕按 [`TILE`] 像素切方块，方块的原始像素无损压缩成一段数据。
//!
//! 算法（2026-10-21 实测选定，见 P1 实施记录第 7 节）：先做颜色变换「减绿」（R−G、B−G，WebP 无损同款），三个平面各自做
//! 中值边缘预测（JPEG-LS 的 MED：左、上、左上三个邻居），残差交给 zstd 1 级。和 PNG 同一结构（先预测、再通用压缩），
//! 照片类画面比 PNG 小一半左右、和 WebP 无损快速档相当，速度更快。每块独立压（方块之间不互相引用），能单独补发、单独撤掉。
//!
//! 数据格式（小端）：版本 1 字节、宽 u16、高 u16、CRC32 u32、zstd 压缩的残差（R−G、G、B−G 三个平面依次排）。
//! CRC32 按预测前的三个平面算（和像素一一对应，两端都现成有，不用再重排一遍）；解出来先核对，对不上就报错——
//! 画进画面的必须和被控端原图逐字节一致。

use std::fmt;

/// 方块边长（像素）。实测 64 / 128 / 整图压缩体积几乎一样，128 块数少、每块开销小
pub const TILE: u32 = 128;
const VERSION: u8 = 1;
const HEADER: usize = 1 + 2 + 2 + 4;
const ZSTD_LEVEL: i32 = 1;

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// 一块屏按 [`TILE`] 切出的方块网格（右边、下边的方块可能不满）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub width: u32,
    pub height: u32,
    pub cols: u32,
    pub rows: u32,
}

impl Grid {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            cols: width.div_ceil(TILE),
            rows: height.div_ceil(TILE),
        }
    }

    pub fn count(&self) -> usize {
        (self.cols * self.rows) as usize
    }

    /// 第 `index` 块在画面里的位置和大小 (x, y, 宽, 高)
    pub fn rect(&self, index: usize) -> (u32, u32, u32, u32) {
        let (col, row) = (index as u32 % self.cols, index as u32 / self.cols);
        let (x, y) = (col * TILE, row * TILE);
        (x, y, TILE.min(self.width - x), TILE.min(self.height - y))
    }
}

/// 把一块 BGRA 像素（逐行紧排，`w`×`h`×4 字节）压缩成数据，追加到 `out` 后面。
pub fn encode(bgra: &[u8], w: u32, h: u32, out: &mut Vec<u8>) -> Result<(), Error> {
    let (wu, hu) = (w as usize, h as usize);
    if w == 0 || h == 0 || w > u16::MAX as u32 || h > u16::MAX as u32 || bgra.len() != wu * hu * 4 {
        return Err(Error(format!("bad tile {w}x{h} with {} bytes", bgra.len())));
    }
    let n = wu * hu;
    // 三个平面：R−G、G、B−G（模 256）
    let mut planes = vec![0u8; n * 3];
    for (i, p) in bgra.chunks_exact(4).enumerate() {
        let (b, g, r) = (p[0], p[1], p[2]);
        planes[i] = r.wrapping_sub(g);
        planes[n + i] = g;
        planes[2 * n + i] = b.wrapping_sub(g);
    }
    let mut residual = vec![0u8; n * 3];
    for c in 0..3 {
        predict(&planes[c * n..(c + 1) * n], &mut residual[c * n..(c + 1) * n], wu, hu);
    }
    let crc = crc32fast::hash(&planes);
    out.push(VERSION);
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    let packed = zstd::bulk::compress(&residual, ZSTD_LEVEL).map_err(|e| Error(format!("zstd: {e}")))?;
    out.extend_from_slice(&packed);
    Ok(())
}

/// 解出一块：返回 (宽, 高, RGBA 像素逐行紧排、alpha 全 255)。CRC32 对不上就报错。
pub fn decode(data: &[u8]) -> Result<(u32, u32, Vec<u8>), Error> {
    if data.len() < HEADER || data[0] != VERSION {
        return Err(Error(format!("bad tile data ({} bytes, version {:?})", data.len(), data.first())));
    }
    let w = u16::from_le_bytes([data[1], data[2]]) as usize;
    let h = u16::from_le_bytes([data[3], data[4]]) as usize;
    let crc = u32::from_le_bytes([data[5], data[6], data[7], data[8]]);
    let n = w * h;
    let residual = zstd::bulk::decompress(&data[HEADER..], n * 3).map_err(|e| Error(format!("zstd: {e}")))?;
    if residual.len() != n * 3 {
        return Err(Error(format!("tile {w}x{h}: {} residual bytes, expected {}", residual.len(), n * 3)));
    }
    let mut planes = vec![0u8; n * 3];
    for c in 0..3 {
        unpredict(&residual[c * n..(c + 1) * n], &mut planes[c * n..(c + 1) * n], w, h);
    }
    if crc32fast::hash(&planes) != crc {
        return Err(Error(format!("tile {w}x{h}: pixels do not match the original (CRC32)")));
    }
    let mut rgba = vec![255u8; n * 4];
    for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
        let g = planes[n + i];
        px[0] = planes[i].wrapping_add(g);
        px[1] = g;
        px[2] = planes[2 * n + i].wrapping_add(g);
    }
    Ok((w as u32, h as u32, rgba))
}

/// 中值边缘预测：a、b、a+b−c 三者的中位数（a 左、b 上、c 左上；边外当 0）
#[inline(always)]
fn med(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (a as i16, b as i16, c as i16);
    (a + b - c).clamp(a.min(b), a.max(b)) as u8
}

/// 第一行只有左邻、第一列只有上邻（另两个当 0，按同一公式算），其余位置三个邻居都在——内层循环不用判断边界
fn predict(src: &[u8], out: &mut [u8], w: usize, h: usize) {
    for y in 0..h {
        let row = y * w;
        let up = if y > 0 { row - w } else { 0 };
        out[row] = src[row].wrapping_sub(med(0, if y > 0 { src[up] } else { 0 }, 0));
        for x in 1..w {
            let (a, b, c) = if y > 0 { (src[row + x - 1], src[up + x], src[up + x - 1]) } else { (src[row + x - 1], 0, 0) };
            out[row + x] = src[row + x].wrapping_sub(med(a, b, c));
        }
    }
}

fn unpredict(res: &[u8], out: &mut [u8], w: usize, h: usize) {
    for y in 0..h {
        let row = y * w;
        let up = if y > 0 { row - w } else { 0 };
        out[row] = res[row].wrapping_add(med(0, if y > 0 { out[up] } else { 0 }, 0));
        if y == 0 {
            for x in 1..w {
                out[x] = res[x].wrapping_add(med(out[x - 1], 0, 0));
            }
        } else {
            let (prev, cur) = out.split_at_mut(row);
            let prev = &prev[up..up + w];
            let cur = &mut cur[..w];
            let res = &res[row..row + w];
            for x in 1..w {
                cur[x] = res[x].wrapping_add(med(cur[x - 1], prev[x], prev[x - 1]));
            }
        }
    }
}
