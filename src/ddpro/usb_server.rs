//! 被控端：USB 透传（P2 第 1 项，`dreamdesk/docs/P2-实施记录.md`）。按连接对账主控端发来的设备全集：新的挂上、没了的撤下。
//!
//! 挂一块设备：开一个只监听 127.0.0.1 的临时端口、只收一个连接，让 usbip-win2 驱动来连（`ddpro_usb::vhci::attach`，
//! 它要等握手经会话连接到主控端走一个来回才返回，所以在单独的线程里）；驱动连上后告诉主控端开一条新流，之后驱动发来的
//! 字节装进 `UsbData` 经会话连接发给主控端，主控端发来的交给这条流自己的写线程写给驱动。流断了（驱动拔下、主控端撤下、
//! 连接关闭）就收拾：拔下驱动端口、停掉驱动的后台重连。
//!
//! 这里的入口都在 RustDesk 连接的异步消息循环里被调（`server.rs`），所以不能有阻塞操作：调驱动命令、写驱动套接字都在线程里，
//! 全局锁里只做不会等的事：查表、对账时开本机临时端口、起线程、往队列里放。
//!
//! 要这条连接能输入（被控端给了键鼠控制权、主控端没开「仅查看」）：挂之前查，运行中变了立刻撤下。

use crate::{
    ddpro::wire::{clip, wrap, PEER_LOG_INTERVAL},
    server::{service::Subscriber, ConnInner},
};
use ddpro::{
    dd_pro,
    usb::vhci,
    usb_status::{Reason, State},
    UsbClose, UsbData, UsbDevice, UsbOpen, UsbStatus,
};
use hbb_common::{log, throttled_log};
use std::{
    collections::{HashMap, HashSet},
    io::{ErrorKind, Read, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, Once,
    },
    time::{Duration, Instant},
};

/// 驱动来连临时端口的时限（握手要经主控端走一个来回，内网几毫秒；这里只防驱动起不来时线程一直挂着）
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(15);
/// 往驱动写一段数据最多等多久（驱动卡住时写线程不跟着永远挂住）
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// 一条连接同时透传的设备最多这么多（每块一个端口、两个线程、一个驱动命令进程，数量由对端决定）
const MAX_DEVICES: usize = 4;

struct Dev {
    device_id: u32,
    busid: String,
    stream_id: u32,
    /// 驱动连上来之后：往这条流的写线程送数据的队列，和用来断开的套接字
    to_driver: Option<(mpsc::Sender<Vec<u8>>, TcpStream)>,
    /// 驱动给的端口号（挂上之后才有），拔下时用
    port_no: Arc<Mutex<Option<u32>>>,
    withdrawn: Arc<AtomicBool>,
}

struct Conn {
    inner: ConnInner,
    keyboard: bool,
    offer: Vec<UsbDevice>,
    devs: Vec<Dev>,
    next_stream: u32,
    /// 已经报过拒绝或挂载失败的设备：不在每次对账时重报、重试；它离开设备全集（拔掉）或权限恢复后清掉
    refused: HashSet<u32>,
}

lazy_static::lazy_static! {
    static ref CONNS: Mutex<HashMap<i32, Conn>> = Default::default();
}

static CLEANUP: Once = Once::new();

fn send(inner: &ConnInner, u: dd_pro::Union) {
    if let Some(m) = wrap(u) {
        inner.clone().send(Arc::new(m));
    }
}

fn status(device_id: u32, state: State, reason: Reason, detail: String) -> dd_pro::Union {
    dd_pro::Union::UsbStatus(UsbStatus {
        device_id,
        state: state.into(),
        reason: reason.into(),
        detail,
        ..Default::default()
    })
}

/// 首次使用：驱动的自动重连必须关掉（见 `ddpro_usb::vhci` 开头；安装时已设，这里兜底：有人重装过驱动等），
/// 拔掉上次进程没来得及拔的（只可能是我们挂的：127.0.0.1 上的）。要调好几条命令，不能在连接的消息循环里跑
fn cleanup() {
    match vhci::ensure_no_reattach() {
        Ok(false) => {}
        Ok(true) => log::warn!(
            "ddpro: usb: the USB/IP driver's automatic reattach was on; turned it off (takes effect after a restart)"
        ),
        Err(e) => log::warn!("ddpro: usb: cannot check the USB/IP driver's automatic reattach setting: {e}"),
    }
    for n in vhci::loopback_ports() {
        log::info!("ddpro: usb: detaching leftover port {n}");
        if let Err(e) = vhci::detach(n) {
            log::warn!("ddpro: usb: cannot detach leftover port {n}: {e}");
        }
    }
}

/// 处理主控端发来的 USB 透传消息。在连接的消息循环里调，只做查表、对账、往队列里放
pub fn on_message(union: dd_pro::Union, conn: &ConnInner, keyboard: bool) {
    let id = conn.id();
    match union {
        dd_pro::Union::UsbOffer(offer) => {
            if CLEANUP.is_completed() {
                on_offer(id, conn, keyboard, offer.devices);
            } else {
                // 第一次：先清理再对账（清理会拔掉 127.0.0.1 上的端口，不能和这次的挂载并行），都在线程里
                let conn = conn.clone();
                let spawned = std::thread::Builder::new().name("ddpro-usb-cleanup".into()).spawn(move || {
                    CLEANUP.call_once(cleanup);
                    on_offer(id, &conn, keyboard, offer.devices);
                });
                if let Err(e) = spawned {
                    log::error!("ddpro: usb: cannot spawn the cleanup thread: {e}");
                }
            }
        }
        dd_pro::Union::UsbData(d) => {
            let conns = CONNS.lock().unwrap();
            let dev = conns
                .get(&id)
                .and_then(|c| c.devs.iter().find(|x| x.stream_id == d.stream_id));
            if let Some((tx, _)) = dev.and_then(|x| x.to_driver.as_ref()) {
                tx.send(d.data).ok();
            }
        }
        dd_pro::Union::UsbClose(c) => {
            let conns = CONNS.lock().unwrap();
            let dev = conns
                .get(&id)
                .and_then(|x| x.devs.iter().find(|d| d.stream_id == c.stream_id));
            if let Some((_, s)) = dev.and_then(|d| d.to_driver.as_ref()) {
                throttled_log!(
                    PEER_LOG_INTERVAL,
                    info,
                    "ddpro: usb: the controller closed stream {}: {}",
                    c.stream_id,
                    clip(&c.detail)
                );
                s.shutdown(Shutdown::Both).ok();
            }
        }
        _ => {}
    }
}

fn on_offer(id: i32, conn: &ConnInner, keyboard: bool, mut devices: Vec<UsbDevice>) {
    if devices.len() > MAX_DEVICES {
        throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: usb: the controller offered {} devices; taking the first {MAX_DEVICES}", devices.len());
        devices.truncate(MAX_DEVICES);
    }
    let mut conns = CONNS.lock().unwrap();
    let c = conns.entry(id).or_insert_with(|| Conn {
        inner: conn.clone(),
        keyboard,
        offer: Vec::new(),
        devs: Vec::new(),
        next_stream: 0,
        refused: HashSet::new(),
    });
    c.keyboard = keyboard;
    let ids: HashSet<u32> = devices.iter().map(|d| d.device_id).collect();
    c.refused.retain(|d| ids.contains(d));
    c.offer = devices;
    reconcile(id, c);
}

/// 这条连接能不能输入变了（被控端切键鼠控制权、主控端开关「仅查看」）
pub fn on_keyboard_permission(conn_id: i32, enabled: bool) {
    let mut conns = CONNS.lock().unwrap();
    if let Some(c) = conns.get_mut(&conn_id) {
        if c.keyboard == enabled {
            return;
        }
        c.keyboard = enabled;
        if enabled {
            c.refused.clear();
        }
        reconcile(conn_id, c);
    }
}

/// 连接关了：全部撤下
pub fn on_conn_closed(conn_id: i32) {
    if let Some(mut c) = CONNS.lock().unwrap().remove(&conn_id) {
        for d in c.devs.drain(..) {
            withdraw(&d);
        }
    }
}

fn reconcile(conn_id: i32, c: &mut Conn) {
    let blocked = if !c.keyboard {
        Some((Reason::NO_PERMISSION, "this connection cannot control the keyboard/mouse".to_owned()))
    } else if vhci::exe().is_none() {
        Some((Reason::DRIVER_MISSING, "the USB/IP driver (usbip-win2) is not installed".to_owned()))
    } else {
        None
    };
    let offered: HashSet<u32> = c.offer.iter().map(|d| d.device_id).collect();
    let (keep, gone): (Vec<Dev>, Vec<Dev>) = c
        .devs
        .drain(..)
        .partition(|d| blocked.is_none() && offered.contains(&d.device_id));
    c.devs = keep;
    for d in gone {
        let (reason, detail) = blocked.clone().unwrap_or((Reason::WITHDRAWN, "withdrawn by the controller".to_owned()));
        log::info!("ddpro: usb: detaching {} ({detail})", d.busid);
        send(&c.inner, status(d.device_id, State::DETACHED, reason, detail));
        if blocked.is_some() {
            c.refused.insert(d.device_id);
        }
        withdraw(&d);
    }
    for dev in c.offer.clone() {
        if c.devs.iter().any(|d| d.device_id == dev.device_id) || c.refused.contains(&dev.device_id) {
            continue;
        }
        if let Some((reason, detail)) = &blocked {
            log::info!("ddpro: usb: not attaching {} {}: {detail}", dev.busid, clip(&dev.name));
            send(&c.inner, status(dev.device_id, State::REJECTED, *reason, detail.clone()));
            c.refused.insert(dev.device_id);
            continue;
        }
        match start(conn_id, c, &dev) {
            Ok(d) => c.devs.push(d),
            Err(e) => {
                send(&c.inner, status(dev.device_id, State::REJECTED, Reason::ATTACH_FAILED, e.clone()));
                c.refused.insert(dev.device_id);
            }
        }
    }
}

fn withdraw(d: &Dev) {
    d.withdrawn.store(true, Ordering::Release);
    if let Some((_, s)) = &d.to_driver {
        s.shutdown(Shutdown::Both).ok();
    }
}

fn detach(port_no: u32, busid: &str) {
    if let Err(e) = vhci::detach(port_no) {
        log::warn!("ddpro: usb: cannot detach {busid} from port {port_no}: {e}");
    }
}

/// 开临时端口、在后台让驱动来挂这块设备
fn start(conn_id: i32, c: &mut Conn, dev: &UsbDevice) -> Result<Dev, String> {
    if !ddpro::usb::usbip::valid_busid(&dev.busid) {
        return Err(format!("invalid busid {:?}", clip(&dev.busid)));
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    c.next_stream = c.next_stream.wrapping_add(1);
    let d = Dev {
        device_id: dev.device_id,
        busid: dev.busid.clone(),
        stream_id: c.next_stream,
        to_driver: None,
        port_no: Default::default(),
        withdrawn: Default::default(),
    };
    log::info!("ddpro: usb: attaching {} {} (stream {}, port {port})", dev.busid, clip(&dev.name), d.stream_id);
    let (inner, busid, device_id, stream_id) = (c.inner.clone(), d.busid.clone(), d.device_id, d.stream_id);
    let (port_no, withdrawn) = (d.port_no.clone(), d.withdrawn.clone());
    std::thread::Builder::new()
        .name("ddpro-usb-attach".into())
        .spawn(move || match vhci::attach(port, &busid) {
            Ok(n) => {
                *port_no.lock().unwrap() = Some(n);
                if withdrawn.load(Ordering::Acquire) {
                    detach(n, &busid);
                } else {
                    log::info!("ddpro: usb: {busid} attached on port {n}");
                    send(&inner, status(device_id, State::ATTACHED, Reason::NONE, String::new()));
                }
            }
            Err(e) => {
                log::warn!("ddpro: usb: attaching {busid} failed: {e}");
                // 驱动也许已经连上来、转发已经开始：断开，转发线程收尾
                if !withdrawn.swap(true, Ordering::AcqRel) {
                    send(&inner, status(device_id, State::REJECTED, Reason::ATTACH_FAILED, e));
                }
                refuse(conn_id, stream_id, device_id);
            }
        })
        .map_err(|e| e.to_string())?;
    let (inner, busid, port_no, withdrawn) = (c.inner.clone(), d.busid.clone(), d.port_no.clone(), d.withdrawn.clone());
    std::thread::Builder::new()
        .name("ddpro-usb-relay".into())
        .spawn(move || {
            if let Some(s) = accept(&listener, &withdrawn) {
                relay(conn_id, &inner, s, device_id, stream_id, &withdrawn);
            }
            // 收拾：先标记撤下（挂载线程之后才返回的话由它拔下），再拔下、停掉驱动对这个位置的后台重连、从连接里去掉
            withdrawn.store(true, Ordering::Release);
            if let Some(n) = port_no.lock().unwrap().take() {
                detach(n, &busid);
            }
            vhci::stop_reattach(port, &busid);
            log::info!("ddpro: usb: {busid} detached (stream {stream_id})");
            if let Some(c) = CONNS.lock().unwrap().get_mut(&conn_id) {
                c.devs.retain(|d| d.stream_id != stream_id);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(d)
}

/// 挂载失败：记下不再重试，断开这条流（转发线程收尾）
fn refuse(conn_id: i32, stream_id: u32, device_id: u32) {
    if let Some(c) = CONNS.lock().unwrap().get_mut(&conn_id) {
        c.refused.insert(device_id);
        if let Some((_, s)) = c.devs.iter().find(|d| d.stream_id == stream_id).and_then(|d| d.to_driver.as_ref()) {
            s.shutdown(Shutdown::Both).ok();
        }
    }
}

/// 等驱动来连（只收一个连接，收到就关掉监听）
fn accept(listener: &TcpListener, withdrawn: &AtomicBool) -> Option<TcpStream> {
    listener.set_nonblocking(true).ok()?;
    let deadline = Instant::now() + ACCEPT_TIMEOUT;
    while Instant::now() < deadline && !withdrawn.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((s, peer)) if peer.ip().is_loopback() => {
                s.set_nonblocking(false).ok()?;
                s.set_nodelay(true).ok();
                s.set_write_timeout(Some(WRITE_TIMEOUT)).ok();
                return Some(s);
            }
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return None,
        }
    }
    None
}

/// 驱动连上了：起这条流的写线程并登记、告诉主控端开流，之后把驱动发来的字节转给主控端，直到断开
fn relay(conn_id: i32, inner: &ConnInner, mut s: TcpStream, device_id: u32, stream_id: u32, withdrawn: &AtomicBool) {
    let (Ok(mut writer), Ok(closer)) = (s.try_clone(), s.try_clone()) else {
        return;
    };
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    {
        let mut conns = CONNS.lock().unwrap();
        let dev = conns
            .get_mut(&conn_id)
            .and_then(|c| c.devs.iter_mut().find(|d| d.stream_id == stream_id));
        match dev {
            Some(d) if !withdrawn.load(Ordering::Acquire) => d.to_driver = Some((tx, closer)),
            _ => return,
        }
    }
    let spawned = std::thread::Builder::new().name("ddpro-usb-write".into()).spawn(move || {
        for data in rx {
            if writer.write_all(&data).is_err() {
                writer.shutdown(Shutdown::Both).ok();
                break;
            }
        }
    });
    if spawned.is_err() {
        return;
    }
    send(
        inner,
        dd_pro::Union::UsbOpen(UsbOpen {
            stream_id,
            device_id,
            ..Default::default()
        }),
    );
    let mut buf = vec![0u8; 64 * 1024];
    let detail = loop {
        match s.read(&mut buf) {
            Ok(0) => break "the driver closed the connection".to_owned(),
            Ok(n) => send(
                inner,
                dd_pro::Union::UsbData(UsbData {
                    stream_id,
                    data: buf[..n].to_vec(),
                    ..Default::default()
                }),
            ),
            Err(e) => break e.to_string(),
        }
    };
    send(
        inner,
        dd_pro::Union::UsbClose(UsbClose {
            stream_id,
            detail,
            ..Default::default()
        }),
    );
}
