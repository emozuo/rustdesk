//! 验证工具（主控端 Linux）：列出认出来的 USB 数位板和导入应答要用的设备信息。普通用户就能跑。
//! 用法：usb_list

#[cfg(target_os = "linux")]
fn main() {
    let tablets = ddpro_usb::tablet::scan();
    if tablets.is_empty() {
        println!("no USB tablet");
    }
    for t in tablets {
        println!("{} {:04x}:{:04x} {}", t.busid, t.vendor, t.product, t.name);
        match ddpro_usb::stub::device_info(&t.busid) {
            Ok(info) => println!("  {info:?}"),
            Err(e) => println!("  device info: {e}"),
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Linux only");
}
