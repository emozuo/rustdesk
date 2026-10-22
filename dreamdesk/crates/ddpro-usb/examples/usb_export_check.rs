//! 验证工具（主控端 Linux，root）：用本 crate 的模块走一遍导出：开一个 TCP 端口等导入方来连，应答导入握手，把数位板交给
//! 内核 usbip-host（本机套接字对），之后双向搬运 SECS 秒，最后收回。不经 RustDesk，用来单独验证握手和 sysfs 操作。
//! 用法：usb_export_check PORT SECS（busid 取第一块认出来的数位板）

#[cfg(target_os = "linux")]
fn main() {
    use ddpro_usb::{stub, tablet, usbip};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        os::{fd::AsRawFd, unix::net::UnixStream},
        thread,
        time::{Duration, Instant},
    };

    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(3241);
    let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let Some(t) = tablet::scan().into_iter().next() else {
        eprintln!("no USB tablet");
        return;
    };
    println!("tablet {} {}", t.busid, t.name);
    let listener = TcpListener::bind(("0.0.0.0", port)).expect("bind");
    let (mut tcp, peer) = listener.accept().expect("accept");
    tcp.set_nodelay(true).ok();
    println!("connection from {peer}");
    let mut req = [0u8; usbip::REQ_IMPORT_LEN];
    tcp.read_exact(&mut req).expect("read import request");
    let busid = usbip::parse_import_request(&req).expect("import request");
    println!("import request for {busid}");
    if busid != t.busid {
        tcp.write_all(&usbip::import_reject()).ok();
        eprintln!("not our tablet; rejected");
        return;
    }
    let info = stub::device_info(&busid).expect("device info");
    let result = (|| -> std::io::Result<()> {
        stub::bind(&busid)?;
        let (kernel, ours) = UnixStream::pair()?;
        stub::attach(&busid, kernel.as_raw_fd())?;
        drop(kernel);
        tcp.write_all(&usbip::import_reply(&info))?;
        println!("attached; relaying {secs} s");
        let (mut tcp_r, mut ours_w) = (tcp.try_clone()?, ours.try_clone()?);
        let a = thread::spawn(move || std::io::copy(&mut tcp_r, &mut ours_w).ok());
        let (mut ours_r, mut tcp_w) = (ours.try_clone()?, tcp.try_clone()?);
        let b = thread::spawn(move || std::io::copy(&mut ours_r, &mut tcp_w).ok());
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end && !a.is_finished() && !b.is_finished() {
            thread::sleep(Duration::from_millis(200));
        }
        tcp.shutdown(std::net::Shutdown::Both).ok();
        ours.shutdown(std::net::Shutdown::Both).ok();
        a.join().ok();
        b.join().ok();
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("export failed: {e}");
    }
    match stub::release(&busid) {
        Ok(()) => println!("released; bound to usbip-host now: {}", stub::bound(&busid)),
        Err(e) => eprintln!("release: {e}"),
    }
    thread::sleep(Duration::from_secs(1));
    println!("tablets after release: {:?}", tablet::scan().iter().map(|t| &t.busid).collect::<Vec<_>>());
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Linux only");
}
