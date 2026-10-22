//! 主控端 Linux：会话里的 USB 透传（P2 第 1 项，`dreamdesk/docs/P2-实施记录.md`）。每个远程桌面会话一个后台线程：
//! - 每 2 秒认一遍本机的 USB 数位板，设备全集有变化就发给被控端（交出去的设备在本机已经没有 input 节点，扫描看不到，
//!   但只要它的流还在就一直算在全集里，否则被控端会以为拔掉了）；
//! - 被控端驱动来要设备（`UsbOpen`，随后第一段数据是导入请求）：核对 busid，请 root 端点把设备交给内核 usbip-host、
//!   把本机套接字对的一端交给内核，回导入应答，之后两边搬运；
//! - 流结束（被控端拔下、驱动断开）：请 root 端点收回，设备回到本机。会话结束时和 root 端点的连接断开，端点收回所有设备。
//!
//! 发给被控端一律走本会话开始时那条连接的发送端（不用 `Session::send`：它总是发往当前连接，断线重连后旧线程还没退出的
//! 那一会儿，旧流的数据会串进新连接）。

use crate::{
    client::Data,
    ddpro::wire::{wrap, PEER_LOG_INTERVAL},
    ui_session_interface::{InvokeUiSession, Session},
};
use ddpro::{
    dd_pro,
    usb::{helper, stub, tablet, usbip},
    usb_status::State,
    UsbClose, UsbData, UsbDevice, UsbOffer,
};
use hbb_common::{log, throttled_log, tokio::sync::mpsc::UnboundedSender};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::Shutdown,
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{mpsc, Mutex},
    time::{Duration, Instant},
};

/// 多久认一遍本机的数位板（原项目实测的间隔：插上后一两秒内送走，开销可以忽略）
const SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// 同时在等导入请求的流最多这么多（被控端开流的数量由对端决定）
const MAX_WAITING: usize = 4;
/// 对端给的说明文字写进日志前截到这么长
const DETAIL_MAX: usize = 200;

enum Inbound {
    Peer(dd_pro::Union),
    /// 本机这一侧的流结束了（内核断开：设备被拔掉，或 usbip-host 出错）
    LocalClosed(u32),
    /// 同一个会话断线重连，新的透传线程起来了：本线程立刻收拾退出
    Stop,
}

lazy_static::lazy_static! {
    /// 会话号 → 这个会话的透传线程
    static ref WORKERS: Mutex<HashMap<u64, mpsc::Sender<Inbound>>> = Default::default();
}

fn key<T: InvokeUiSession>(session: &Session<T>) -> u64 {
    session.lc.read().unwrap().session_id
}

/// 对端提供专业协议、版本对得上（`client::on_peer_info`）：起这个会话的透传线程。断线重连时换掉旧的
pub fn start<T: InvokeUiSession>(session: &Session<T>) {
    let Some(rustdesk) = session.sender.read().unwrap().clone() else {
        return;
    };
    let (tx, rx) = mpsc::channel();
    if let Some(old) = WORKERS.lock().unwrap().insert(key(session), tx.clone()) {
        old.send(Inbound::Stop).ok();
    }
    let spawned = std::thread::Builder::new().name("ddpro-usb".into()).spawn(move || {
        Worker::new(rustdesk, tx).run(rx);
    });
    if let Err(e) = spawned {
        log::error!("ddpro: usb: cannot spawn the passthrough thread: {e}");
    }
}

/// 被控端发来的 USB 透传消息（`client::on_message`）
pub fn on_message<T: InvokeUiSession>(session: &Session<T>, u: dd_pro::Union) {
    if let Some(tx) = WORKERS.lock().unwrap().get(&key(session)) {
        tx.send(Inbound::Peer(u)).ok();
    }
}

fn usb_data(stream_id: u32, data: Vec<u8>) -> dd_pro::Union {
    dd_pro::Union::UsbData(UsbData {
        stream_id,
        data,
        ..Default::default()
    })
}

fn send(rustdesk: &UnboundedSender<Data>, u: dd_pro::Union) {
    if let Some(m) = wrap(u) {
        rustdesk.send(Data::Message(m)).ok();
    }
}

fn clip(s: &str) -> &str {
    match s.char_indices().nth(DETAIL_MAX) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

enum Stream {
    /// 被控端开了流，在等驱动的导入请求
    Waiting { device_id: u32, buf: Vec<u8> },
    /// 已交给内核：往内核写的一端
    Open { busid: String, to_kernel: UnixStream },
}

struct Worker {
    rustdesk: UnboundedSender<Data>,
    inbox: mpsc::Sender<Inbound>,
    helper: Option<helper::Client>,
    /// busid → 发给被控端的设备（device_id 在本会话内不变）
    known: HashMap<String, UsbDevice>,
    next_id: u32,
    offered: Vec<u32>,
    streams: HashMap<u32, Stream>,
}

impl Worker {
    fn new(rustdesk: UnboundedSender<Data>, inbox: mpsc::Sender<Inbound>) -> Self {
        Self {
            rustdesk,
            inbox,
            helper: None,
            known: HashMap::new(),
            next_id: 0,
            offered: Vec::new(),
            streams: HashMap::new(),
        }
    }

    fn send(&self, u: dd_pro::Union) {
        send(&self.rustdesk, u);
    }

    fn run(mut self, rx: mpsc::Receiver<Inbound>) {
        let mut next_scan = Instant::now();
        while !self.rustdesk.is_closed() {
            // 按截止时间扫：用笔时消息不断，不能每来一条就把扫描往后推
            if Instant::now() >= next_scan {
                self.scan();
                next_scan = Instant::now() + SCAN_INTERVAL;
            }
            match rx.recv_timeout(next_scan.saturating_duration_since(Instant::now())) {
                Ok(Inbound::Peer(u)) => self.peer(u),
                Ok(Inbound::LocalClosed(id)) => self.close(id, "the device side closed", true),
                Ok(Inbound::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        // 会话结束：断开 root 端点，它收回本连接交出去的所有设备
        for (_, s) in self.streams.drain() {
            if let Stream::Open { to_kernel, .. } = s {
                to_kernel.shutdown(Shutdown::Both).ok();
            }
        }
        if self.helper.take().is_some() {
            log::info!("ddpro: usb: session over; the helper returns passed-through devices to this machine");
        }
    }

    /// 设备全集：本机认出来的，加上正在透传的（交出去的在本机看不到）
    fn scan(&mut self) {
        let mut busids: Vec<String> = Vec::new();
        for t in tablet::scan() {
            if !self.known.contains_key(&t.busid) {
                self.next_id += 1;
                log::info!("ddpro: usb: found tablet {} {:04x}:{:04x} {}", t.busid, t.vendor, t.product, t.name);
                let device = UsbDevice {
                    device_id: self.next_id,
                    busid: t.busid.clone(),
                    vendor_id: t.vendor as u32,
                    product_id: t.product as u32,
                    name: t.name.clone(),
                    ..Default::default()
                };
                self.known.insert(t.busid.clone(), device);
            }
            busids.push(t.busid);
        }
        for s in self.streams.values() {
            if let Stream::Open { busid, .. } = s {
                if !busids.contains(busid) {
                    busids.push(busid.clone());
                }
            }
        }
        self.known.retain(|b, _| busids.contains(b));
        let mut ids: Vec<u32> = self.known.values().map(|d| d.device_id).collect();
        ids.sort_unstable();
        if ids != self.offered {
            self.offered = ids;
            self.send(dd_pro::Union::UsbOffer(UsbOffer {
                devices: self.known.values().cloned().collect(),
                ..Default::default()
            }));
        }
    }

    fn peer(&mut self, u: dd_pro::Union) {
        match u {
            dd_pro::Union::UsbStatus(s) => {
                let name = self.name_of(s.device_id);
                match s.state.enum_value_or_default() {
                    State::ATTACHED => log::info!("ddpro: usb: {name} attached on the remote side"),
                    state => throttled_log!(
                        PEER_LOG_INTERVAL,
                        warn,
                        "ddpro: usb: {name} {state:?} ({:?}): {}",
                        s.reason,
                        clip(&s.detail)
                    ),
                }
            }
            dd_pro::Union::UsbOpen(o) => {
                let waiting = self.streams.values().filter(|s| matches!(s, Stream::Waiting { .. })).count();
                if waiting >= MAX_WAITING {
                    throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: usb: too many streams opened by the remote side");
                    self.send(dd_pro::Union::UsbClose(UsbClose {
                        stream_id: o.stream_id,
                        detail: "too many streams".to_owned(),
                        ..Default::default()
                    }));
                    return;
                }
                self.streams.insert(
                    o.stream_id,
                    Stream::Waiting {
                        device_id: o.device_id,
                        buf: Vec::new(),
                    },
                );
            }
            dd_pro::Union::UsbData(d) => self.data(d.stream_id, &d.data),
            dd_pro::Union::UsbClose(c) => self.close(c.stream_id, clip(&c.detail), false),
            _ => {}
        }
    }

    fn name_of(&self, device_id: u32) -> String {
        self.known
            .values()
            .find(|d| d.device_id == device_id)
            .map_or_else(|| format!("device {device_id}"), |d| format!("{} {}", d.busid, d.name))
    }

    fn data(&mut self, stream_id: u32, data: &[u8]) {
        match self.streams.get_mut(&stream_id) {
            Some(Stream::Open { to_kernel, .. }) => {
                if to_kernel.write_all(data).is_err() {
                    self.close(stream_id, "cannot write to the device", true);
                }
            }
            Some(Stream::Waiting { device_id, buf }) => {
                buf.extend_from_slice(data);
                if buf.len() < usbip::REQ_IMPORT_LEN {
                    return;
                }
                let (device_id, buf) = (*device_id, std::mem::take(buf));
                if let Err(e) = self.export(stream_id, device_id, &buf) {
                    log::warn!("ddpro: usb: cannot pass {} through: {e}", self.name_of(device_id));
                    self.send(usb_data(stream_id, usbip::import_reject()));
                    self.close(stream_id, &e, true);
                }
            }
            None => {}
        }
    }

    /// 驱动的导入请求到了：核对、请 root 端点交给内核、回导入应答、开始搬运
    fn export(&mut self, stream_id: u32, device_id: u32, buf: &[u8]) -> Result<(), String> {
        let busid = usbip::parse_import_request(buf)?;
        let name = self
            .known
            .get(&busid)
            .filter(|d| d.device_id == device_id)
            .map(|d| d.name.clone())
            .ok_or_else(|| format!("the remote side asked for {busid}, which is not offered as device {device_id}"))?;
        if self.streams.values().any(|s| matches!(s, Stream::Open { busid: b, .. } if *b == busid)) {
            return Err(format!("{busid} is already passed through"));
        }
        // 导入应答要的设备信息：交出去之前现读（缓存的话，两次扫描之间同一个口拔插过，设备号就对不上了）
        let info = stub::device_info(&busid).map_err(|e| format!("cannot read {busid}: {e}"))?;
        if self.helper.is_none() {
            let c = helper::Client::connect(&super::usb_helper::socket_path(), |fd| {
                crate::ipc::peer_uid_from_fd(fd) == Some(0)
            })
            .map_err(|e| format!("cannot reach the USB helper of the system service: {e}"))?;
            self.helper = Some(c);
        }
        self.helper_call(|h| h.bind(&busid))?;
        // 绑上之后（设备已离开本机）出任何错都收回，设备回到本机
        let opened = (|| -> Result<(UnixStream, UnixStream), String> {
            let (kernel, ours) = UnixStream::pair().map_err(|e| e.to_string())?;
            self.helper_call(|h| h.attach(&busid, kernel.as_raw_fd()))?;
            drop(kernel);
            let to_kernel = ours.try_clone().map_err(|e| e.to_string())?;
            Ok((ours, to_kernel))
        })();
        let (ours, to_kernel) = match opened {
            Ok(v) => v,
            Err(e) => {
                self.release(&busid);
                return Err(e);
            }
        };
        self.send(usb_data(stream_id, usbip::import_reply(&info)));
        // 导入请求后面万一跟着别的字节（正常不会），原样交给内核
        if buf.len() > usbip::REQ_IMPORT_LEN {
            (&to_kernel).write_all(&buf[usbip::REQ_IMPORT_LEN..]).ok();
        }
        log::info!("ddpro: usb: passing {busid} {name} through (stream {stream_id})");
        self.streams.insert(
            stream_id,
            Stream::Open {
                busid: busid.clone(),
                to_kernel,
            },
        );
        let (rustdesk, inbox) = (self.rustdesk.clone(), self.inbox.clone());
        let spawned = std::thread::Builder::new().name("ddpro-usb-relay".into()).spawn(move || {
            let mut ours = ours;
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = ours.read(&mut buf) {
                if n == 0 {
                    break;
                }
                send(&rustdesk, usb_data(stream_id, buf[..n].to_vec()));
            }
            inbox.send(Inbound::LocalClosed(stream_id)).ok();
        });
        if let Err(e) = spawned {
            self.close(stream_id, "cannot start relaying", true);
            return Err(e.to_string());
        }
        Ok(())
    }

    /// 调 root 端点；出错就丢掉这条连接（应答可能已经错位，或 root 服务重启了），下次重连。丢掉连接时 root 端点会收回
    /// 本会话交出去的所有设备——出错时宁可都还给本机
    fn helper_call(&mut self, f: impl FnOnce(&mut helper::Client) -> std::io::Result<()>) -> Result<(), String> {
        let h = self.helper.as_mut().ok_or("no USB helper")?;
        f(h).map_err(|e| {
            if e.kind() != std::io::ErrorKind::Other {
                self.helper = None;
            }
            e.to_string()
        })
    }

    fn release(&mut self, busid: &str) {
        if self.helper.is_none() {
            return;
        }
        if let Err(e) = self.helper_call(|h| h.release(busid)) {
            log::warn!("ddpro: usb: returning {busid} to this machine failed: {e}");
        }
    }

    /// 结束一条流：收回设备（回到本机）；`tell_peer`：是本机这边先断的，告诉被控端
    fn close(&mut self, stream_id: u32, detail: &str, tell_peer: bool) {
        let Some(s) = self.streams.remove(&stream_id) else {
            return;
        };
        if tell_peer {
            self.send(dd_pro::Union::UsbClose(UsbClose {
                stream_id,
                detail: detail.to_owned(),
                ..Default::default()
            }));
        }
        if let Stream::Open { busid, to_kernel } = s {
            to_kernel.shutdown(Shutdown::Both).ok();
            self.release(&busid);
            log::info!("ddpro: usb: {busid} back on this machine (stream {stream_id}: {detail})");
        }
    }
}
