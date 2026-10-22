//! USB 透传（P2 第 1 项）里和 RustDesk 无关的部分：USB/IP 导入握手的报文、主控端 Linux 上认数位板和操作内核
//! `usbip-host`、被控端 Windows 上调 usbip-win2。和会话、连接打交道的接线在主 crate 的 `src/ddpro/usb_*`。
//!
//! 数据通路：主控端的数位板 → 内核 usbip-host ⇄ 本机套接字对 ⇄ 会话进程 ⇄（RustDesk 会话连接，专业协议信封）⇄
//! 被控端 `--server` ⇄ 127.0.0.1 临时端口 ⇄ usbip-win2 驱动。

pub mod usbip;

#[cfg(target_os = "linux")]
pub mod fdpass;
#[cfg(target_os = "linux")]
pub mod helper;
#[cfg(target_os = "linux")]
pub mod stub;
#[cfg(target_os = "linux")]
pub mod tablet;

#[cfg(windows)]
pub mod vhci;
