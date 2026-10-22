//! 主控端 Linux（要 root）：把设备交给 / 收回内核 usbip-host（sysfs 接口，内核 `drivers/usb/usbip/stub_main.c`、
//! `stub_dev.c`）。和 `usbip bind` 做的一样：登记 `match_busid`、从现在的驱动解绑、绑到 usbip-host；再把一个已连上的
//! 流式套接字的描述符写进 `usbip_sockfd`，内核就在这个套接字上收发 URB（实验 E1：AF_UNIX 套接字对也接受）。

pub use crate::usbip::valid_busid;
use crate::usbip::{speed_from_sysfs, DeviceInfo};
use std::{fs, io, os::fd::RawFd, path::PathBuf, process::Command};

const HOST: &str = "/sys/bus/usb/drivers/usbip-host";

fn dev(busid: &str) -> PathBuf {
    PathBuf::from("/sys/bus/usb/devices").join(busid)
}

fn write(path: impl AsRef<std::path::Path>, s: &str) -> io::Result<()> {
    fs::write(path, s)
}

fn invalid(busid: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("invalid busid {busid:?}"))
}

/// 导入应答要的设备信息。必须在从本机驱动解绑之前读：解绑后配置值、接口数读出来是空的（实验 E1 撞到）
pub fn device_info(busid: &str) -> io::Result<DeviceInfo> {
    if !valid_busid(busid) {
        return Err(invalid(busid));
    }
    let d = dev(busid);
    let text = |f: &str| fs::read_to_string(d.join(f)).map(|s| s.trim().to_owned());
    let num = |f: &str| -> io::Result<u32> {
        text(f)?
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("{busid}/{f} is not a number")))
    };
    let hex = |f: &str| -> io::Result<u32> {
        u32::from_str_radix(&text(f)?, 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("{busid}/{f} is not hex")))
    };
    Ok(DeviceInfo {
        path: fs::canonicalize(&d)?.to_string_lossy().into_owned(),
        busid: busid.to_owned(),
        busnum: num("busnum")?,
        devnum: num("devnum")?,
        speed: speed_from_sysfs(&text("speed")?),
        vendor: hex("idVendor")? as u16,
        product: hex("idProduct")? as u16,
        bcd_device: hex("bcdDevice")? as u16,
        class: hex("bDeviceClass")? as u8,
        subclass: hex("bDeviceSubClass")? as u8,
        protocol: hex("bDeviceProtocol")? as u8,
        configuration: num("bConfigurationValue").unwrap_or(1) as u8,
        num_configurations: num("bNumConfigurations")? as u8,
        num_interfaces: num("bNumInterfaces")? as u8,
    })
}

/// 现在绑在 usbip-host 上
pub fn bound(busid: &str) -> bool {
    valid_busid(busid)
        && fs::canonicalize(dev(busid).join("driver")).is_ok_and(|p| p.file_name().is_some_and(|n| n == "usbip-host"))
}

/// 从本机驱动解绑、交给 usbip-host（设备从本机消失）
pub fn bind(busid: &str) -> io::Result<()> {
    if !valid_busid(busid) {
        return Err(invalid(busid));
    }
    if !std::path::Path::new(HOST).exists() {
        let st = Command::new("modprobe").arg("usbip-host").status()?;
        if !st.success() {
            return Err(io::Error::other(format!("modprobe usbip-host failed: {st}")));
        }
    }
    write(format!("{HOST}/match_busid"), &format!("add {busid}"))?;
    let d = dev(busid);
    if d.join("driver").exists() && !bound(busid) {
        write(d.join("driver/unbind"), busid)?;
    }
    if !bound(busid) {
        write(format!("{HOST}/bind"), busid)?;
    }
    Ok(())
}

/// 把已连上的流式套接字交给内核（描述符必须在本进程里）；之后内核在它上面收发 URB
pub fn attach(busid: &str, fd: RawFd) -> io::Result<()> {
    if !valid_busid(busid) {
        return Err(invalid(busid));
    }
    write(dev(busid).join("usbip_sockfd"), &fd.to_string())
}

/// 收回：断开内核的连接、从 usbip-host 解绑、撤掉 `match_busid` 登记、让本机驱动重新认领。顺序照 Linux 自带的
/// `usbip unbind`：解绑 → `del` → 写 `rebind`。只写 `del` 清不掉登记（解绑后它处在「已登记、未绑」状态，内核只把它标成待删），
/// 要 `rebind` 让本机驱动认领之后内核才真删；登记留着的话，这个 USB 口以后插什么都会先被 usbip-host 抢走（2026-10-22 实测撞到）。
/// 每一步都尽量做，返回第一个错误
pub fn release(busid: &str) -> io::Result<()> {
    if !valid_busid(busid) {
        return Err(invalid(busid));
    }
    let d = dev(busid);
    let mut first = None;
    let mut note = |r: io::Result<()>| {
        if let Err(e) = r {
            first.get_or_insert(e);
        }
    };
    // 连接已经断了时内核会拒绝（EINVAL），不算错
    if bound(busid) {
        write(d.join("usbip_sockfd"), "-1").ok();
        note(write(format!("{HOST}/unbind"), busid));
    }
    if std::path::Path::new(HOST).exists() && registered(busid) {
        note(write(format!("{HOST}/match_busid"), &format!("del {busid}")));
        if d.exists() {
            note(write(format!("{HOST}/rebind"), busid));
        }
        // 设备已经拔掉（rebind 无从做起）、或别的驱动早就认领了：这时登记处在可直接删的状态，再删一次
        if registered(busid) {
            write(format!("{HOST}/match_busid"), &format!("del {busid}")).ok();
        }
    }
    if d.exists() && !d.join("driver").exists() {
        note(write("/sys/bus/usb/drivers_probe", busid));
    }
    first.map_or(Ok(()), Err)
}

/// `match_busid` 里还登记着这个 busid
pub fn registered(busid: &str) -> bool {
    fs::read_to_string(format!("{HOST}/match_busid")).is_ok_and(|s| s.split_whitespace().any(|b| b == busid))
}
