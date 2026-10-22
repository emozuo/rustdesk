//! 验证工具（主控端 Linux）：root 端点和普通用户客户端分开跑，验证权限分离下的整条导出。
//!   usb_helper_check serve SOCKET STATE_DIR —— root 跑端点（验证用，不做授权）
//!   usb_helper_check client SOCKET PORT SECS [kill] —— 普通用户跑：等导入方连 TCP 端口，经端点绑定、交套接字、搬运 SECS 秒、
//!                                            收回；带 kill 时不收回直接退出，看端点是否在连接断开后自己收回

#[cfg(target_os = "linux")]
fn main() {
    use ddpro_usb::{helper, stub, tablet, usbip};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        os::{fd::AsRawFd, unix::net::UnixStream},
        path::Path,
        thread,
        time::{Duration, Instant},
    };

    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("serve") => {
            helper::serve(Path::new(&a[2]), Path::new(&a[3]), |_| true, |m| println!("{m}")).expect("serve");
        }
        Some("client") => {
            let port: u16 = a[3].parse().unwrap();
            let secs: u64 = a[4].parse().unwrap();
            let kill = a.get(5).is_some_and(|s| s == "kill");
            let mut c = helper::Client::connect(Path::new(&a[2]), |_| true).expect("connect helper");
            let t = tablet::scan().into_iter().next().expect("no tablet");
            println!("uid {} tablet {}", unsafe { libc_getuid() }, t.busid);
            // 普通用户自己不能绑：先确认一下 sysfs 拒绝
            println!("direct bind as this user: {:?}", stub::bind(&t.busid).err().map(|e| e.kind()));
            let (mut tcp, peer) = TcpListener::bind(("0.0.0.0", port)).unwrap().accept().unwrap();
            println!("connection from {peer}");
            let mut req = [0u8; usbip::REQ_IMPORT_LEN];
            tcp.read_exact(&mut req).unwrap();
            let busid = usbip::parse_import_request(&req).unwrap();
            let info = stub::device_info(&busid).expect("device info");
            c.bind(&busid).expect("helper bind");
            let (kernel, ours) = UnixStream::pair().unwrap();
            c.attach(&busid, kernel.as_raw_fd()).expect("helper attach");
            drop(kernel);
            tcp.write_all(&usbip::import_reply(&info)).unwrap();
            println!("attached via helper; relaying {secs} s");
            let (mut tr, mut ow) = (tcp.try_clone().unwrap(), ours.try_clone().unwrap());
            thread::spawn(move || std::io::copy(&mut tr, &mut ow).ok());
            let (mut or, mut tw) = (ours.try_clone().unwrap(), tcp.try_clone().unwrap());
            thread::spawn(move || std::io::copy(&mut or, &mut tw).ok());
            let end = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < end {
                thread::sleep(Duration::from_millis(200));
            }
            if kill {
                println!("exiting without release");
                std::process::exit(0);
            }
            tcp.shutdown(std::net::Shutdown::Both).ok();
            c.release(&busid).expect("helper release");
            thread::sleep(Duration::from_secs(1));
            println!("released; tablets now {:?}", tablet::scan().iter().map(|t| &t.busid).collect::<Vec<_>>());
        }
        _ => eprintln!("usage: usb_helper_check serve SOCKET STATE | client SOCKET PORT SECS [kill]"),
    }

    unsafe fn libc_getuid() -> u32 {
        extern "C" {
            fn getuid() -> u32;
        }
        getuid()
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Linux only");
}
