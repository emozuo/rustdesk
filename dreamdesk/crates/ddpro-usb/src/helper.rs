//! 主控端 Linux：root 服务里的 USB 特权端点，和会话进程那一侧的客户端。
//!
//! 会话进程是普通用户，绑定 / 解绑驱动、写 `usbip_sockfd` 要 root，所以由 root 的 `--service` 开一个 Unix 套接字代做，
//! 只有三个操作：
//! - `BIND busid`：把设备从本机驱动解绑、交给 usbip-host（root 自己再认一遍它是数位板，不信会话进程说的）；
//! - `ATTACH busid`（随消息经 SCM_RIGHTS 带一个已连上的流式套接字）：交给内核；
//! - `RELEASE busid`：收回，还给本机驱动。
//!
//! 按连接记账：一条连接绑的设备只有它能操作，连接一断（会话结束、会话进程崩溃）立刻全部收回；另一条连接要同一块设备
//! 先到先得。绑着的设备同时记进状态文件，root 服务重启时先收回上次没收回的（只收回这里记过的，不碰别人手工导出的）。
//! 状态文件放在 root 自己建的 0700 目录里（不放在套接字那个 /tmp 下的目录：那里在 RustDesk 收归 root 之前别人可能先建、
//! 事先放好指向系统文件的链接，root 一写就覆盖了它）。
//! 授权（只认当前活动会话用户的同一个程序）由主程序传进来，用的是 RustDesk 自己的检查。
//!
//! 线协议：一行一个请求、一行一个应答（`OK` 或 `ERR 原因`）；请求一来一回，不并发。

use crate::{fdpass, stub, tablet};
use std::{
    collections::HashMap,
    fs, io,
    os::{
        fd::{AsRawFd, OwnedFd, RawFd},
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

/// 同时接受的连接数（每个远程会话一条，正常只有一两条）
const MAX_CONNS: usize = 8;
const MAX_LINE: usize = 128;

/// 按设备记它属于哪条连接
#[derive(Default)]
struct Owners {
    by_busid: HashMap<String, u64>,
    state: PathBuf,
}

impl Owners {
    fn save(&self, log: &dyn Fn(&str)) {
        let list: Vec<&str> = self.by_busid.keys().map(String::as_str).collect();
        if let Err(e) = fs::write(&self.state, list.join("\n")) {
            log(&format!("usb helper: cannot write {}: {e}; a restart of the service will not release these", self.state.display()));
        }
    }
}

/// root 端点：监听 `path`，状态文件放在 `state_dir`（root 建、0700），每条连接一个线程。不返回。
/// `path` 的父目录必须已经是 root 所有、别人不可写的真目录（[`parent_ready`]），否则报错
pub fn serve(
    path: &Path,
    state_dir: &Path,
    authorize: impl Fn(RawFd) -> bool + Send + Sync + 'static,
    log: impl Fn(&str) + Send + Sync + 'static,
) -> io::Result<()> {
    let log = Arc::new(log);
    parent_ready(path)?;
    let state = private_dir(state_dir)?.join("bound");
    // 上次没收回的（root 服务重启前绑着的）先收回
    if let Ok(s) = fs::read_to_string(&state) {
        for busid in s.lines().filter(|l| stub::valid_busid(l)) {
            if stub::bound(busid) || stub::registered(busid) {
                match stub::release(busid) {
                    Ok(()) => log(&format!("released {busid} left bound by a previous run")),
                    Err(e) => log(&format!("cannot release {busid} left bound by a previous run: {e}")),
                }
            }
        }
    }
    fs::write(&state, "")?;
    // 目录别人不可写，这里删掉的只可能是上次留下的套接字
    fs::remove_file(path).ok();
    let listener = UnixListener::bind(path)?;
    // 谁都能连，授权在每条连接上做（和 RustDesk 的 `_drm` 一样）
    fs::set_permissions(path, fs::Permissions::from_mode(0o666))?;
    log(&format!("usb helper listening at {}", path.display()));
    let owners = Arc::new(Mutex::new(Owners {
        by_busid: HashMap::new(),
        state,
    }));
    let authorize = Arc::new(authorize);
    let conns = Arc::new(AtomicUsize::new(0));
    let mut next_id = 0u64;
    for s in listener.incoming() {
        let Ok(s) = s else {
            continue;
        };
        if !authorize(s.as_raw_fd()) {
            continue;
        }
        if conns.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
            conns.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        next_id += 1;
        let (id, o, c, l) = (next_id, owners.clone(), conns.clone(), log.clone());
        let spawned = std::thread::Builder::new().name("ddpro-usb-helper".into()).spawn(move || {
            handle(id, s, &o, &*l);
            c.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            // 线程起不来时闭包连同连接一起被丢掉，计数要还回去
            conns.fetch_sub(1, Ordering::SeqCst);
            log("usb helper: cannot spawn a connection thread");
        }
    }
    Ok(())
}

/// 套接字所在目录是 root 所有、组和其他人不可写的真目录（不是符号链接）：否则别人可以先占这个路径冒充端点。
/// RustDesk 的 `_service` 通道起来时会把它收归 root，起得比本端点晚时调用方等它
pub fn parent_ready(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let dir = path.parent().ok_or_else(|| io::Error::other("socket path has no parent"))?;
    let m = fs::symlink_metadata(dir)?;
    if !m.is_dir() || m.uid() != 0 || m.mode() & 0o022 != 0 {
        return Err(io::Error::other(format!(
            "{} must be a directory owned by root and not group/world writable",
            dir.display()
        )));
    }
    Ok(())
}

/// root 自己的 0700 目录（没有就建），确认是真目录、root 所有、别人进不去
fn private_dir(dir: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    if let Err(e) = fs::DirBuilder::new().mode(0o700).create(dir) {
        if e.kind() != io::ErrorKind::AlreadyExists {
            return Err(e);
        }
    }
    let m = fs::symlink_metadata(dir)?;
    if !m.is_dir() || m.uid() != 0 || m.mode() & 0o077 != 0 {
        return Err(io::Error::other(format!("{} must be a root-only (0700) directory", dir.display())));
    }
    Ok(dir.to_owned())
}

fn handle(id: u64, s: UnixStream, owners: &Mutex<Owners>, log: &dyn Fn(&str)) {
    let mut mine: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    loop {
        let (line, fd) = match read_line(&s, &mut buf) {
            Ok(Some(v)) => v,
            _ => break,
        };
        let reply = match request(&line, fd, &mut mine, id, owners, log) {
            Ok(()) => "OK\n".to_owned(),
            Err(e) => {
                log(&format!("usb helper: {line}: {e}"));
                format!("ERR {}\n", e.replace('\n', " "))
            }
        };
        if fdpass::send(s.as_raw_fd(), reply.as_bytes(), None).is_err() {
            break;
        }
    }
    // 连接断了（会话结束、会话进程崩溃）：本连接绑的全部收回
    let mut o = owners.lock().unwrap();
    for busid in mine {
        if let Err(e) = stub::release(&busid) {
            log(&format!("usb helper: release {busid} after the session ended: {e}"));
        }
        o.by_busid.remove(&busid);
    }
    o.save(log);
}

fn request(
    line: &str,
    fd: Option<OwnedFd>,
    mine: &mut Vec<String>,
    id: u64,
    owners: &Mutex<Owners>,
    log: &dyn Fn(&str),
) -> Result<(), String> {
    let (cmd, busid) = line.split_once(' ').ok_or("malformed request")?;
    if !stub::valid_busid(busid) {
        return Err("invalid busid".into());
    }
    let mut o = owners.lock().unwrap();
    let owner = o.by_busid.get(busid).copied();
    match cmd {
        "BIND" => {
            if owner.is_some() {
                return Err("the device is already passed through".into());
            }
            if !tablet::scan().iter().any(|t| t.busid == busid) {
                return Err("not a USB tablet".into());
            }
            o.by_busid.insert(busid.to_owned(), id);
            mine.push(busid.to_owned());
            o.save(log);
            // 绑定可能做了一半（已从本机驱动解绑、没绑上 usbip-host）：就地收回，设备回到本机，所有权也撤掉
            if let Err(e) = stub::bind(busid) {
                if let Err(r) = stub::release(busid) {
                    log(&format!("usb helper: release {busid} after a failed bind: {r}"));
                }
                o.by_busid.remove(busid);
                mine.retain(|b| b != busid);
                o.save(log);
                return Err(e.to_string());
            }
            Ok(())
        }
        "ATTACH" => {
            if owner != Some(id) || !stub::bound(busid) {
                return Err("bind the device first".into());
            }
            let fd = fd.ok_or("no socket attached")?;
            stub::attach(busid, fd.as_raw_fd()).map_err(|e| e.to_string())
        }
        "RELEASE" => {
            if owner != Some(id) {
                return Ok(());
            }
            o.by_busid.remove(busid);
            mine.retain(|b| b != busid);
            o.save(log);
            stub::release(busid).map_err(|e| e.to_string())
        }
        _ => Err("unknown request".into()),
    }
}

/// 读一行（到换行为止），途中带来的描述符一并取出
fn read_line(s: &UnixStream, buf: &mut Vec<u8>) -> io::Result<Option<(String, Option<OwnedFd>)>> {
    let mut fd = None;
    loop {
        if let Some(i) = buf.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&buf[..i]).trim().to_owned();
            buf.drain(..=i);
            return Ok(Some((line, fd)));
        }
        if buf.len() > MAX_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
        }
        let mut chunk = [0u8; MAX_LINE];
        let (n, got) = fdpass::recv(s.as_raw_fd(), &mut chunk)?;
        if n == 0 {
            return Ok(None);
        }
        if got.is_some() {
            fd = got;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// 会话进程那一侧：连上 root 端点，一问一答
pub struct Client {
    s: UnixStream,
    buf: Vec<u8>,
}

impl Client {
    /// `is_root`：核对对端确实是 root（防止别人先占了路径冒充端点）
    pub fn connect(path: &Path, is_root: impl Fn(RawFd) -> bool) -> io::Result<Self> {
        let s = UnixStream::connect(path)?;
        if !is_root(s.as_raw_fd()) {
            return Err(io::Error::other("the USB helper is not running as root"));
        }
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        Ok(Self { s, buf: Vec::new() })
    }

    fn call(&mut self, cmd: &str, busid: &str, fd: Option<RawFd>) -> io::Result<()> {
        fdpass::send(self.s.as_raw_fd(), format!("{cmd} {busid}\n").as_bytes(), fd)?;
        let (line, _) = read_line(&self.s, &mut self.buf)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "the USB helper closed the connection"))?;
        match line.strip_prefix("ERR ") {
            Some(e) => Err(io::Error::other(e.to_owned())),
            None if line == "OK" => Ok(()),
            None => Err(io::Error::other(format!("unexpected reply {line:?}"))),
        }
    }

    pub fn bind(&mut self, busid: &str) -> io::Result<()> {
        self.call("BIND", busid, None)
    }

    pub fn attach(&mut self, busid: &str, sock: RawFd) -> io::Result<()> {
        self.call("ATTACH", busid, Some(sock))
    }

    pub fn release(&mut self, busid: &str) -> io::Result<()> {
        self.call("RELEASE", busid, None)
    }
}
