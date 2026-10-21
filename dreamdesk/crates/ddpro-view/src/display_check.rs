//! 显示输出这一段会不会改颜色（P1 第 5 项第 2 步）。合成器把画面交给内核之后，内核还按显示接口和显示控制器的属性
//! 处理像素：送有限范围（16–235）、过伽马曲线（GNOME 的夜灯、颜色配置文件的校准曲线都写在这里）、乘颜色矩阵。
//! 直接读这些属性比逐个查桌面设置准——不管是谁改的，改了就看得到。只读，普通用户就能读，不需要显示主控权，不改任何设置。

use drm::control::{property, Device as ControlDevice, ResourceHandle};
use std::{
    fs::{self, File, OpenOptions},
    os::fd::{AsFd, BorrowedFd},
};

/// 伽马曲线和恒等曲线差多少以内算原样：不到 8 位输出的半级（65535 / 255 / 2），8 位画面经过它不变
const LUT_TOLERANCE: u32 = 128;

struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl drm::Device for Card {}
impl ControlDevice for Card {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Issue {
    /// 显示接口送有限范围 RGB：设成了有限，或「自动」遇上电视分辨率
    LimitedRange,
    /// 伽马曲线不是原样（夜灯、颜色配置文件的校准曲线）
    GammaCurve,
    /// 颜色矩阵或反伽马曲线不是原样
    ColorTransform,
}

/// 一块在用的屏的检查结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputCheck {
    /// 内核的显示接口名，如 `DP-2`
    pub connector: String,
    pub issues: Vec<Issue>,
    /// 读到的原始值（写日志用）
    pub detail: String,
}

/// 本机的显卡（`/dev/dri/card*`），打开一次、之后反复查。
///
/// 只在合成器正占着显示主控权时打开（专业窗口已经开出来了就是）：显卡此刻没有主控者时，内核让第一个打开它的进程
/// 自动当主控者——每次查都重新打开的话，碰上切虚拟终端那一瞬间就可能把主控权抢过来，合成器回来时拿不回屏幕。
pub struct Cards(Vec<(String, Card)>);

impl Cards {
    pub fn open() -> Result<Self, String> {
        let dir = fs::read_dir("/dev/dri").map_err(|e| format!("read /dev/dri: {e}"))?;
        let mut cards = Vec::new();
        let mut errors = Vec::new();
        for entry in dir.flatten() {
            if !entry.file_name().to_string_lossy().starts_with("card") {
                continue;
            }
            let path = entry.path();
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .or_else(|_| File::open(&path));
            match file {
                Ok(f) => {
                    let card = Card(f);
                    // 多显卡的主控端：没被合成器占着的那块显卡没有主控者，打开它就自动成了主控者，会挡住别的程序用它。
                    // 只读属性不需要主控权，立刻放掉（不是主控者时内核报错，不用管；5.8 起的内核允许自己放掉）
                    drm::Device::release_master_lock(&card).ok();
                    cards.push((path.display().to_string(), card))
                }
                Err(e) => errors.push(format!("open {}: {e}", path.display())),
            }
        }
        if cards.is_empty() {
            return Err(if errors.is_empty() {
                "no display card under /dev/dri".to_owned()
            } else {
                errors.join("; ")
            });
        }
        Ok(Self(cards))
    }

    /// 查在用的屏。`want` 是合成器给的屏名：能对上内核显示接口名的只报那一块，对不上就全报。
    /// 一块在用的屏都没读到时报错（不当成「没问题」）。
    pub fn check(&self, want: Option<&str>) -> Result<Vec<OutputCheck>, String> {
        let mut all = Vec::new();
        let mut errors = Vec::new();
        for (path, card) in &self.0 {
            match check_card(card) {
                Ok(v) => all.extend(v),
                Err(e) => errors.push(format!("{path}: {e}")),
            }
        }
        if all.is_empty() {
            return Err(if errors.is_empty() {
                "no active display found".to_owned()
            } else {
                errors.join("; ")
            });
        }
        if let Some(want) = want {
            let want = normalize(want);
            if all.iter().any(|c| normalize(&c.connector) == want) {
                all.retain(|c| normalize(&c.connector) == want);
            }
        }
        Ok(all)
    }
}

/// 内核叫 `HDMI-A-1`，合成器可能叫 `HDMI-1`
fn normalize(name: &str) -> String {
    name.replace("HDMI-A-", "HDMI-")
}

fn check_card(card: &Card) -> std::io::Result<Vec<OutputCheck>> {
    let res = card.resource_handles()?;
    let mut out = Vec::new();
    for &h in res.connectors() {
        let info = card.get_connector(h, false)?;
        let Some(enc) = info.current_encoder() else {
            continue;
        };
        let Some(crtc) = card.get_encoder(enc)?.crtc() else {
            continue;
        };
        let Some(mode) = card.get_crtc(crtc)?.mode() else {
            continue;
        };
        let connector = format!("{}-{}", info.interface().as_str(), info.interface_id());
        let mut issues = Vec::new();
        let mut detail = Vec::new();

        let rgb = props(card, h)?
            .into_iter()
            .find(|(n, ..)| n == "Broadcast RGB")
            .and_then(|(_, p, raw)| enum_name(&p, raw));
        let (w, hgt) = mode.size();
        let tv = tv_mode(&mode);
        detail.push(format!(
            "{w}x{hgt} {} kHz{}, Broadcast RGB {}",
            mode.clock(),
            if tv { " (TV mode)" } else { "" },
            rgb.as_deref().unwrap_or("n/a")
        ));
        // i915 的「自动」：HDMI、DP（不含 eDP）遇上电视分辨率送有限范围（drm_default_rgb_quant_range）
        let hdmi_or_dp = matches!(
            info.interface(),
            drm::control::connector::Interface::HDMIA
                | drm::control::connector::Interface::HDMIB
                | drm::control::connector::Interface::DisplayPort
        );
        match rgb.as_deref() {
            Some(v) if v.starts_with("Limited") => issues.push(Issue::LimitedRange),
            Some("Automatic") if tv && hdmi_or_dp => issues.push(Issue::LimitedRange),
            _ => {}
        }

        let mut color_transform = false;
        for (name, _, raw) in props(card, crtc)? {
            match name.as_str() {
                "GAMMA_LUT" => {
                    let dev = lut_deviation(card, raw)?;
                    detail.push(format!("GAMMA_LUT deviation {dev}/65535"));
                    if dev > LUT_TOLERANCE {
                        issues.push(Issue::GammaCurve);
                    }
                }
                "DEGAMMA_LUT" => {
                    let dev = lut_deviation(card, raw)?;
                    if dev > LUT_TOLERANCE {
                        detail.push(format!("DEGAMMA_LUT deviation {dev}/65535"));
                        color_transform = true;
                    }
                }
                "CTM" if raw != 0 && !ctm_identity(&card.get_property_blob(raw)?) => {
                    detail.push("CTM set".to_owned());
                    color_transform = true;
                }
                _ => {}
            }
        }
        if color_transform {
            issues.push(Issue::ColorTransform);
        }
        out.push(OutputCheck {
            connector,
            issues,
            detail: detail.join(", "),
        });
    }
    Ok(out)
}

fn props<H: ResourceHandle>(card: &Card, h: H) -> std::io::Result<Vec<(String, property::Info, u64)>> {
    let set = card.get_properties(h)?;
    let (ids, values) = set.as_props_and_values();
    let mut v = Vec::with_capacity(ids.len());
    for (&id, &raw) in ids.iter().zip(values) {
        let info = card.get_property(id)?;
        v.push((info.name().to_string_lossy().into_owned(), info, raw));
    }
    Ok(v)
}

fn enum_name(p: &property::Info, raw: u64) -> Option<String> {
    match p.value_type() {
        property::ValueType::Enum(e) => e
            .get_value_from_raw_value(raw)
            .map(|v| v.name().to_string_lossy().into_owned()),
        _ => None,
    }
}

/// 伽马 / 反伽马曲线和恒等曲线最多差多少（满量程 65535）；没设（0）就是恒等
fn lut_deviation(card: &Card, blob: u64) -> std::io::Result<u32> {
    if blob == 0 {
        return Ok(0);
    }
    // 每项是 drm_color_lut：红、绿、蓝、保留，各 16 位
    let data = card.get_property_blob(blob)?;
    let n = data.len() / 8;
    let mut worst = 0;
    for (i, e) in data.chunks_exact(8).enumerate() {
        let ideal = (i as u64 * 65535 + (n.max(2) as u64 - 1) / 2) / (n.max(2) as u64 - 1);
        for c in 0..3 {
            let v = u16::from_le_bytes([e[c * 2], e[c * 2 + 1]]) as u64;
            worst = worst.max(v.abs_diff(ideal) as u32);
        }
    }
    Ok(worst)
}

/// 颜色矩阵是不是单位阵：3×3 个 S31.32 定点数（最高位是符号，其余是绝对值）
fn ctm_identity(blob: &[u8]) -> bool {
    if blob.len() < 72 {
        return false;
    }
    (0..9).all(|i| {
        let v = u64::from_le_bytes(blob[i * 8..i * 8 + 8].try_into().unwrap_or_default());
        let magnitude = v & !(1 << 63);
        if i % 4 == 0 {
            v == 1 << 32
        } else {
            magnitude == 0
        }
    })
}

/// 是不是电视分辨率（CTA-861 里 VIC 大于 1 的格式）：按有效区、总行列和像素时钟比对常用的几种，
/// 像素时钟允许 1000/1001 的变体（59.94 Hz 等）
fn tv_mode(mode: &drm::control::Mode) -> bool {
    // (宽, 高, 每行总像素, 总行数, 像素时钟 kHz)
    const CTA: &[(u16, u16, u16, u16, u32)] = &[
        (720, 480, 858, 525, 27_000),
        (720, 576, 864, 625, 27_000),
        (1280, 720, 1650, 750, 74_250),
        (1280, 720, 1980, 750, 74_250),
        (1920, 1080, 2200, 1125, 148_500),
        (1920, 1080, 2640, 1125, 148_500),
        (1920, 1080, 2750, 1125, 74_250),
        (1920, 1080, 2640, 1125, 74_250),
        (1920, 1080, 2200, 1125, 74_250),
        (1920, 1080, 2200, 1125, 297_000),
        (1920, 1080, 2640, 1125, 297_000),
        (3840, 2160, 5500, 2250, 297_000),
        (3840, 2160, 5280, 2250, 297_000),
        (3840, 2160, 4400, 2250, 297_000),
        (3840, 2160, 5280, 2250, 594_000),
        (3840, 2160, 4400, 2250, 594_000),
        (4096, 2160, 5500, 2250, 297_000),
        (4096, 2160, 5280, 2250, 297_000),
        (4096, 2160, 4400, 2250, 297_000),
        (4096, 2160, 5280, 2250, 594_000),
        (4096, 2160, 4400, 2250, 594_000),
    ];
    let (w, h) = mode.size();
    let htotal = mode.hsync().2;
    let vtotal = mode.vsync().2;
    let clock = mode.clock();
    CTA.iter().any(|&(cw, ch, ht, vt, ck)| {
        (cw, ch, ht, vt) == (w, h, htotal, vtotal)
            && [ck, ck * 1000 / 1001].iter().any(|&c| clock.abs_diff(c) <= c / 1000)
    })
}
