//! 主控端会话进程这一侧：拉起专业窗口进程（`--pro-view`），经它的标准输入 / 输出交换消息。
//!
//! 用子进程的管道而不是 RustDesk 的进程间通道：消息里有画面通道的密钥，子进程的管道只有父子两个进程碰得到；
//! 会话结束时这边关掉管道，专业窗口读到断开自己退出（P0 实施记录第 5 步第 4 项）。
//! 专业窗口交回的键鼠走 RustDesk 原有的输入入口（和 Flutter 窗口同一个入口、同一种键码）。

use super::wire::PEER_LOG_INTERVAL;
use crate::{
    client::{Data, Interface},
    ui_session_interface::{InvokeUiSession, Session},
};
use base::message_proto::{message, misc, KeyboardMode, Message, PeerInfo};
use ddpro::{
    view_down, view_up, Chroma, DisplayCaps, FormatCaps, MediaChannel, VideoFormat, ViewDown,
    ViewStart, ViewUp,
};
use hbb_common::{log, protobuf::Message as _, throttled_log, tokio::sync::mpsc::UnboundedSender};
use std::{
    collections::HashMap,
    io::BufReader,
    process::{Command, Stdio},
    sync::{
        mpsc::{self, RecvTimeoutError},
        Mutex, OnceLock,
    },
    time::Duration,
};

/// 每个会话（按 RustDesk 的会话号）：远程当前屏的编号和位置尺寸，专业窗口开着时还有给它发消息的通道。
#[derive(Default)]
struct Link {
    index: i32,
    display: (i32, i32, u32, u32),
    /// 专业窗口的进程号和给它发消息的通道。换屏时旧窗口还没退出新窗口就开了，旧窗口收尾时凭进程号
    /// 只清自己的
    down: Option<(u32, mpsc::Sender<ViewDown>)>,
}

static LINKS: OnceLock<Mutex<HashMap<u64, Link>>> = OnceLock::new();

fn links() -> &'static Mutex<HashMap<u64, Link>> {
    LINKS.get_or_init(Default::default)
}

fn key<T: InvokeUiSession>(session: &Session<T>) -> u64 {
    session.lc.read().unwrap().session_id
}

/// 收到对端信息时记下远程当前屏的编号和位置尺寸（专业窗口换算鼠标坐标要用）。
pub fn remember_display<T: InvokeUiSession>(pi: &PeerInfo, session: &Session<T>) {
    let rect = usize::try_from(pi.current_display)
        .ok()
        .and_then(|i| pi.displays.get(i))
        .map(|d| (d.x, d.y, d.width.max(0) as u32, d.height.max(0) as u32))
        .unwrap_or_default();
    let mut links = links().lock().unwrap();
    let link = links.entry(key(session)).or_default();
    link.index = pi.current_display;
    link.display = rect;
}

/// 本机能不能硬解 4:4:4 并在显卡上转 RGB（只探测一次）。
pub fn decode_caps() -> FormatCaps {
    static CAPS: OnceLock<FormatCaps> = OnceLock::new();
    CAPS.get_or_init(|| {
        let probe = ddpro::dec::Device::open().and_then(|d| d.probe());
        match probe {
            Ok(c) if c.hevc_444_8bit && c.video_proc => {
                log::info!("ddpro: this machine can decode HEVC 4:4:4 8bit in hardware ({c:?})");
                FormatCaps {
                    checked: true,
                    formats: vec![VideoFormat {
                        chroma: Chroma::CHROMA_444.into(),
                        bit_depth: 8,
                        ..Default::default()
                    }],
                    ..Default::default()
                }
            }
            Ok(c) => {
                log::warn!("ddpro: no pro mode: the GPU lacks HEVC 4:4:4 decode or video processing ({c:?})");
                FormatCaps {
                    checked: true,
                    ..Default::default()
                }
            }
            Err(e) => {
                log::warn!("ddpro: no pro mode: hardware decoding is unavailable: {e}");
                FormatCaps {
                    checked: true,
                    ..Default::default()
                }
            }
        }
    })
    .clone()
}

/// 本机能不能做专业显示（Wayland 桌面、合成器能用我们的画面格式；只探测一次）。
pub fn display_caps() -> DisplayCaps {
    static CAPS: OnceLock<DisplayCaps> = OnceLock::new();
    CAPS.get_or_init(|| {
        let available = match ddpro::view::probe() {
            Ok(()) => true,
            Err(e) => {
                log::warn!("ddpro: no pro mode: the pro view cannot be shown here: {e}");
                false
            }
        };
        DisplayCaps {
            checked: true,
            available,
            ..Default::default()
        }
    })
    .clone()
}

/// 被控端同意后拉起专业窗口。`rustdesk` 是 RustDesk 消息循环的发送端，它关了说明会话已结束。
pub fn start<T: InvokeUiSession>(
    media: &MediaChannel,
    rustdesk: UnboundedSender<Data>,
    session: &Session<T>,
) {
    let k = key(session);
    let display = links()
        .lock()
        .unwrap()
        .get(&k)
        .map(|l| l.display)
        .unwrap_or_default();
    if display.2 == 0 || display.3 == 0 {
        log::error!("ddpro: cannot start the pro view: the remote display size is unknown");
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            log::error!("ddpro: cannot start the pro view: {e}");
            return;
        }
    };
    let mut child = match Command::new(exe)
        .arg("--pro-view")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log::error!("ddpro: cannot start the pro view: {e}");
            return;
        }
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        log::error!("ddpro: the pro view has no pipes");
        child.kill().ok();
        return;
    };
    log::info!("ddpro: pro view started (pid {})", child.id());

    let (tx, rx) = mpsc::channel::<ViewDown>();
    let down = |union| ViewDown {
        union: Some(union),
        ..Default::default()
    };
    tx.send(down(view_down::Union::Start(ViewStart {
        media: Some(media.clone()).into(),
        remote_x: display.0,
        remote_y: display.1,
        remote_width: display.2,
        remote_height: display.3,
        title: format!("{} - pro", session.get_id()),
        ..Default::default()
    })))
    .ok();
    // 远程光标图案对端只发一次，之后只发编号：把已经收到的都先给专业窗口，后面的编号才对得上
    for cd in session.cursor_shapes.read().unwrap().values() {
        let mut m = Message::new();
        m.set_cursor_data(cd.clone());
        if let Ok(bytes) = m.write_to_bytes() {
            tx.send(down(view_down::Union::Rustdesk(bytes))).ok();
        }
    }
    let pid = child.id();
    links().lock().unwrap().entry(k).or_default().down = Some((pid, tx));

    // 写：启动参数、光标；会话结束或专业窗口结束时停，关掉管道
    std::thread::spawn(move || loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(m) => {
                if let Err(e) = ddpro::write_frame(&mut stdin, &m) {
                    log::warn!("ddpro: cannot write to the pro view: {e}");
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if rustdesk.is_closed() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    });

    // 读：键鼠交给 RustDesk 原有的输入入口；专业窗口退出后收尾
    let session = session.clone();
    std::thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        let mut warned_mode = false;
        let reason = loop {
            let msg = match ddpro::read_frame::<ViewUp>(&mut stdout) {
                Ok(Some(m)) => m,
                Ok(None) => break "the pro view exited".to_owned(),
                Err(e) => break format!("lost the pro view: {e}"),
            };
            match msg.union {
                Some(view_up::Union::Key(k)) => {
                    // P0 只支持映射模式：选了别的模式时专业窗口里的按键按映射处理
                    let mut mode = session.get_keyboard_mode();
                    let map = KeyboardMode::Map.to_string();
                    if mode != map {
                        if !warned_mode {
                            warned_mode = true;
                            log::warn!("ddpro: keyboard mode {mode} is not supported in the pro view yet; using {map}");
                        }
                        mode = map;
                    }
                    session.handle_flutter_raw_key_event(
                        &mode,
                        "",
                        0,
                        k.position_code,
                        k.lock_modes,
                        k.down,
                    );
                }
                Some(view_up::Union::Mouse(m)) => {
                    session.send_mouse(m.mask, m.x, m.y, m.alt, m.ctrl, m.shift, m.command);
                }
                Some(view_up::Union::Ended(r)) => log::info!("ddpro: the pro view is ending: {r}"),
                _ => {}
            }
        };
        let status = child.wait();
        log::info!("ddpro: pro view closed ({reason}); exit status {status:?}");
        if let Some(l) = links().lock().unwrap().get_mut(&k) {
            if l.down.as_ref().is_some_and(|(p, _)| *p == pid) {
                l.down = None;
            }
        }
    });
}

/// RustDesk 连接里收到的消息里专业窗口要的（RustDesk 自己的处理照旧）：
/// - 光标：原样转给开着的专业窗口；
/// - 显示变化（和 RustDesk 一样当作当前屏的变化）：同一块屏的记下新位置尺寸、原样转给专业窗口；
///   换到了别的屏而专业窗口开着，关掉它，返回新屏的编号，由调用方对新屏重新发起专业请求。
pub fn on_rustdesk_message<T: InvokeUiSession>(msg: &Message, session: &Session<T>) -> Option<i32> {
    let mut links = links().lock().unwrap();
    match &msg.union {
        Some(message::Union::CursorData(_)) | Some(message::Union::CursorId(_)) => {}
        Some(message::Union::Misc(m)) => {
            let Some(misc::Union::SwitchDisplay(s)) = &m.union else {
                return None;
            };
            let link = links.entry(key(session)).or_default();
            let same = s.display == link.index;
            link.index = s.display;
            if s.width > 0 && s.height > 0 {
                link.display = (s.x, s.y, s.width as u32, s.height as u32);
            }
            if !same {
                // 丢掉通道，写线程随之关管道，专业窗口读到断开就告诉被控端不要了、自己退出
                return link.down.take().map(|_| s.display);
            }
        }
        _ => return None,
    }
    let (_, down) = links.get(&key(session))?.down.as_ref()?;
    match msg.write_to_bytes() {
        Ok(bytes) => {
            down.send(ViewDown {
                union: Some(view_down::Union::Rustdesk(bytes)),
                ..Default::default()
            })
            .ok();
        }
        Err(e) => throttled_log!(
            PEER_LOG_INTERVAL,
            warn,
            "ddpro: cannot forward a message to the pro view: {e}"
        ),
    }
    None
}
