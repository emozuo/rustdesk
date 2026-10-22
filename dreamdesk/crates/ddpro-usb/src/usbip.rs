//! USB/IP 导入握手（Linux 内核文档 `Documentation/usb/usbip_protocol.rst`，全部大端）：被控端的导入方先发 40 字节的
//! OP_REQ_IMPORT（8 字节头 + 32 字节 busid），主控端回 OP_REP_IMPORT（8 字节头 + 312 字节设备信息）；之后就是 URB 报文，
//! 由内核 usbip-host 处理，原样转发。不用 usbipd：它只做这一问一答和把套接字交给内核两件事。

pub const VERSION: u16 = 0x0111;
const OP_REQ_IMPORT: u16 = 0x8003;
const OP_REP_IMPORT: u16 = 0x0003;
pub const REQ_IMPORT_LEN: usize = 40;
const DEVICE_INFO_LEN: usize = 312;
pub const REP_IMPORT_LEN: usize = 8 + DEVICE_INFO_LEN;

/// 导入应答里的设备信息，字段和内核 `struct usbip_usb_device` 一一对应
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    /// sysfs 路径
    pub path: String,
    pub busid: String,
    pub busnum: u32,
    pub devnum: u32,
    /// `enum usb_device_speed`
    pub speed: u32,
    pub vendor: u16,
    pub product: u16,
    pub bcd_device: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub configuration: u8,
    pub num_configurations: u8,
    pub num_interfaces: u8,
}

/// busid 只能是 Linux 的「总线-端口[.端口…]」（如 1-1、3-2.4）：主控端 root 要把它拼进 sysfs 路径，被控端要把它当命令行参数
/// 传给驱动，都不能让对端借此塞进别的东西（`..`、`-` 这类只含点和横线的也不行）
pub fn valid_busid(busid: &str) -> bool {
    let num = |s: &str| !s.is_empty() && s.len() <= 3 && s.bytes().all(|b| b.is_ascii_digit());
    let Some((bus, ports)) = busid.split_once('-') else {
        return false;
    };
    busid.len() < 32 && num(bus) && ports.split('.').all(num)
}

/// 解析导入请求，返回要导入的 busid。版本不对、不是导入请求（比如设备列表请求）都算错
pub fn parse_import_request(req: &[u8]) -> Result<String, String> {
    if req.len() < REQ_IMPORT_LEN {
        return Err(format!("import request too short ({} bytes)", req.len()));
    }
    let version = u16::from_be_bytes([req[0], req[1]]);
    let code = u16::from_be_bytes([req[2], req[3]]);
    if version != VERSION || code != OP_REQ_IMPORT {
        return Err(format!("not a USB/IP import request (version {version:#06x}, code {code:#06x})"));
    }
    let busid = &req[8..REQ_IMPORT_LEN];
    let end = busid.iter().position(|&b| b == 0).unwrap_or(busid.len());
    String::from_utf8(busid[..end].to_vec()).map_err(|_| "busid is not UTF-8".to_owned())
}

/// 成功的导入应答
pub fn import_reply(info: &DeviceInfo) -> Vec<u8> {
    let mut out = Vec::with_capacity(REP_IMPORT_LEN);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&OP_REP_IMPORT.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    put_str(&mut out, &info.path, 256);
    put_str(&mut out, &info.busid, 32);
    for v in [info.busnum, info.devnum, info.speed] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    for v in [info.vendor, info.product, info.bcd_device] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&[
        info.class,
        info.subclass,
        info.protocol,
        info.configuration,
        info.num_configurations,
        info.num_interfaces,
    ]);
    debug_assert_eq!(out.len(), REP_IMPORT_LEN);
    out
}

/// 拒绝导入：只回 8 字节头，状态非 0（和 usbipd 失败时一样，导入方据此报挂载失败）
pub fn import_reject() -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&OP_REP_IMPORT.to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    out
}

/// 定长、以 0 补齐的字符串字段（超长截断，至少留一个 0）
fn put_str(out: &mut Vec<u8>, s: &str, len: usize) {
    let b = s.as_bytes();
    let n = b.len().min(len - 1);
    out.extend_from_slice(&b[..n]);
    out.resize(out.len() + len - n, 0);
}

/// sysfs `speed` 文件（Mbps）→ `enum usb_device_speed`
pub fn speed_from_sysfs(s: &str) -> u32 {
    match s.trim() {
        "1.5" => 1,
        "12" => 2,
        "480" => 3,
        "53.3-480" => 4,
        "5000" => 5,
        "10000" | "20000" => 6,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_layout() {
        let info = DeviceInfo {
            path: "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-1".into(),
            busid: "1-1".into(),
            busnum: 1,
            devnum: 2,
            speed: 2,
            vendor: 0x056a,
            product: 0x0375,
            bcd_device: 0x0100,
            configuration: 1,
            num_configurations: 1,
            num_interfaces: 3,
            ..Default::default()
        };
        let r = import_reply(&info);
        assert_eq!(r.len(), 320);
        assert_eq!(&r[..8], &[0x01, 0x11, 0x00, 0x03, 0, 0, 0, 0]);
        assert_eq!(&r[8 + 256..8 + 256 + 4], b"1-1\0");
        // 头 8 + 路径 256 + busid 32 = 296 起：busnum、devnum、speed，再是 vid、pid、bcdDevice，最后 6 个单字节
        assert_eq!(&r[296..308], &[0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 2]);
        assert_eq!(&r[308..314], &[0x05, 0x6a, 0x03, 0x75, 0x01, 0x00]);
        assert_eq!(&r[314..], &[0, 0, 0, 1, 1, 3]);
    }

    #[test]
    fn busid() {
        for ok in ["1-1", "3-2.4", "12-1.2.3"] {
            assert!(valid_busid(ok), "{ok}");
        }
        for bad in ["", "-", "..", "1-", "-1", "1-1.", "1-.1", "1-1-1", "a-1", "1-1/..", "1--1"] {
            assert!(!valid_busid(bad), "{bad}");
        }
    }

    #[test]
    fn request() {
        let mut req = vec![0x01, 0x11, 0x80, 0x03, 0, 0, 0, 0];
        req.extend_from_slice(b"1-1");
        req.resize(40, 0);
        assert_eq!(parse_import_request(&req).as_deref(), Ok("1-1"));
        req[3] = 0x05;
        assert!(parse_import_request(&req).is_err());
    }
}
