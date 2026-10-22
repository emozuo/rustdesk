//! 主控端 Linux：认出插着的 USB 数位板。判据沿用原项目实测踩出来的：input 节点有笔（BTN_TOOL_PEN）、走 USB 总线
//! （蓝牙板不认；只看「有没有笔」，手柄、3D 鼠标不会误认）。只读 sysfs，普通用户就能跑。

use std::{
    fs,
    path::{Path, PathBuf},
};

const BTN_TOOL_PEN: usize = 0x140;
const BUS_USB: u64 = 0x0003;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tablet {
    pub busid: String,
    pub vendor: u16,
    pub product: u16,
    pub name: String,
}

/// 现在插着的 USB 数位板，按 busid 排序
pub fn scan() -> Vec<Tablet> {
    let mut out: Vec<Tablet> = Vec::new();
    let Ok(dir) = fs::read_dir("/sys/class/input") else {
        return out;
    };
    for e in dir.flatten() {
        if !e.file_name().to_string_lossy().starts_with("input") {
            continue;
        }
        let p = e.path();
        if read_hex(&p.join("id/bustype")) != Some(BUS_USB) || !has_key(&p.join("capabilities/key"), BTN_TOOL_PEN) {
            continue;
        }
        let Some(busid) = usb_device_of(&p) else {
            continue;
        };
        if out.iter().any(|t| t.busid == busid) {
            continue;
        }
        if let Some(t) = describe(&busid) {
            out.push(t);
        }
    }
    out.sort_by(|a, b| a.busid.cmp(&b.busid));
    out
}

/// input 节点所在的 USB 设备（往上找第一个带 idVendor、名字里没有冒号的目录，冒号的是接口）
fn usb_device_of(input: &Path) -> Option<String> {
    let mut p: PathBuf = fs::canonicalize(input).ok()?;
    while p.pop() {
        let name = p.file_name()?.to_string_lossy().into_owned();
        if !name.contains(':') && p.join("idVendor").exists() {
            return Some(name);
        }
    }
    None
}

fn describe(busid: &str) -> Option<Tablet> {
    let d = Path::new("/sys/bus/usb/devices").join(busid);
    let text = |f: &str| fs::read_to_string(d.join(f)).map(|s| s.trim().to_owned()).unwrap_or_default();
    let name = [text("manufacturer"), text("product")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Some(Tablet {
        busid: busid.to_owned(),
        vendor: u16::from_str_radix(&text("idVendor"), 16).ok()?,
        product: u16::from_str_radix(&text("idProduct"), 16).ok()?,
        name,
    })
}

fn read_hex(p: &Path) -> Option<u64> {
    u64::from_str_radix(fs::read_to_string(p).ok()?.trim(), 16).ok()
}

/// sysfs 的按键能力位图：空格分开的 64 位十六进制字，高位字在前
fn has_key(p: &Path, bit: usize) -> bool {
    let Ok(s) = fs::read_to_string(p) else {
        return false;
    };
    let words: Vec<&str> = s.split_whitespace().rev().collect();
    words
        .get(bit / 64)
        .and_then(|w| u64::from_str_radix(w, 16).ok())
        .is_some_and(|w| w & (1 << (bit % 64)) != 0)
}
