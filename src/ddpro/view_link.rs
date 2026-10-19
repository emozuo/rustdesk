//! 主控端会话进程这一侧：拉起专业窗口进程（`--pro-view`），经它的标准输入 / 输出交换消息。
//!
//! 用子进程的管道而不是 RustDesk 的进程间通道：消息里有画面通道的密钥，子进程的管道只有父子两个进程碰得到；
//! 会话结束时这边关掉管道，专业窗口读到断开自己退出（P0 实施记录第 5 步第 4 项）。
//! 专业窗口交回的键鼠走 RustDesk 原有的输入入口（和 Flutter 窗口同一个入口、同一种键码）。
//!
//! 「非专业模式」提示（不静默降级，P0 实施记录「P0 收尾：模式提示」）也在这里：对端支持专业模式、但这次会话
//! 现在不在专业模式时，往 RustDesk 远程窗口推 `ddpro_mode` 事件，界面在画面左上角挂常驻标签。

use super::wire::PEER_LOG_INTERVAL;
use crate::{
    client::{Data, Interface},
    ui_session_interface::{InvokeUiSession, Session},
};
use base::message_proto::{message, misc, KeyboardMode, Message, PeerInfo};
use ddpro::{
    reject, stopped, view_down, view_ended::Kind, view_up, Chroma, DisplayCaps, FormatCaps,
    MediaChannel, Reject, Stopped, VideoFormat, ViewDown, ViewEnded, ViewStart, ViewUp,
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
    /// 这次专业请求（开会话、换屏各算一次）挂着的提示：只报第一个原因，后面的多半是它的连带结果；
    /// 例外是主控端因「没画面」推测的通道原因，被控端随后报来的确切原因可以替换它
    notice: Option<&'static str>,
}

static LINKS: OnceLock<Mutex<HashMap<u64, Link>>> = OnceLock::new();

fn links() -> &'static Mutex<HashMap<u64, Link>> {
    LINKS.get_or_init(Default::default)
}

fn key<T: InvokeUiSession>(session: &Session<T>) -> u64 {
    session.lc.read().unwrap().session_id
}

// 「非专业模式」提示的结论。英文本身是多语言表的键（`src/lang`），界面按它翻译
const NOT_PRO_PEER_FAILED: &str = "Not in pro mode: the controlled side failed to capture or encode";
const NOT_PRO_NO_IMAGE: &str = "Not in pro mode: the controlled side's screen shows no picture (no monitor connected?)";
const NOT_PRO_LOST: &str = "Not in pro mode: the pro channel was interrupted";
const NOT_PRO_CHANNEL_FAILED: &str = "Not in pro mode: the pro channel could not be opened";
const NOT_PRO_DISPLAY: &str = "Not in pro mode: this computer cannot show the pro view";
const NOT_PRO_UNSUPPORTED: &str = "Not in pro mode: this computer does not support pro mode";
const NOT_PRO_PEER_UNAVAILABLE: &str = "Not in pro mode: the controlled side cannot provide pro video";
const NOT_PRO_BUSY: &str = "Not in pro mode: this display is already in use";
const NOT_PRO_VERSION: &str = "Not in pro mode: the pro protocol versions do not match";
const NOT_PRO_USER_CLOSED: &str = "Not in pro mode: you closed the pro view";

/// 每种结论配一句说明：这意味着什么、该怎么处理（鼠标停在标签上显示，原始错误跟在后面）。同样是多语言表的键
fn tip(text: &str) -> &'static str {
    match text {
        NOT_PRO_PEER_FAILED => "The controlled side could not capture the screen or encode the pro video. Reconnect to try again; if it keeps happening, check the controlled side's graphics driver.",
        NOT_PRO_NO_IMAGE => "The controlled side has no monitor connected, or its monitor is asleep, so the graphics card produces no picture. Connect a monitor (or an HDMI dummy plug), or enable a virtual display on the controlled side, then reconnect.",
        NOT_PRO_LOST => "No pro video arrived for 3 seconds, usually because the network was interrupted. Reconnect after the network recovers.",
        NOT_PRO_CHANNEL_FAILED => "The pro video channel could not be set up. A firewall between the two computers may be blocking UDP. Check the network or firewall, then reconnect.",
        NOT_PRO_DISPLAY => "This computer failed to show the pro video (hardware decoding or display error). Reconnect to try again; if it keeps happening, check this computer's graphics driver and desktop.",
        NOT_PRO_UNSUPPORTED => "This computer does not meet the pro mode requirements: hardware HEVC 4:4:4 decoding and a Wayland desktop.",
        NOT_PRO_PEER_UNAVAILABLE => "The controlled side cannot provide pro video, for example because its graphics card cannot encode HEVC 4:4:4.",
        NOT_PRO_BUSY => "Another connection is using this display of the controlled side (pro mode allows one connection per display). Reconnect after it ends, or switch to another display.",
        NOT_PRO_VERSION => "The two computers run different versions of the pro protocol. Upgrade the older side so both run the same version.",
        NOT_PRO_USER_CLOSED => "You closed the pro view, so this session is shown in general mode. Reconnect to return to pro mode.",
        _ => "",
    }
}

/// 推给 RustDesk 远程窗口：`level` 为 `fault`（红）、`user`（灰），空串表示撤掉提示
fn push_mode<T: InvokeUiSession>(session: &Session<T>, level: &str, text: &str, detail: &str) {
    let conn_type = session.lc.read().unwrap().conn_type;
    if let Some(s) = crate::flutter::sessions::get_session_by_peer_id(session.get_id(), conn_type) {
        s.push_event(
            "ddpro_mode",
            &[("level", level), ("text", text), ("tip", tip(text)), ("detail", detail)],
            &[],
        );
    }
}

/// 发起一次专业请求（开会话、换屏）：这次请求里的第一个故障才挂提示。
pub fn begin_attempt<T: InvokeUiSession>(session: &Session<T>) {
    links().lock().unwrap().entry(key(session)).or_default().notice = None;
}

/// 这次专业请求失败：写日志、挂红色提示（同一次请求只报第一个原因）。
pub fn fault<T: InvokeUiSession>(session: &Session<T>, text: &'static str, detail: &str) {
    raise(session, text, detail, |_| false);
}

/// 挂红色提示；这次请求已经挂着提示时，只有 `replaces` 认可的才替换。
fn raise<T: InvokeUiSession>(
    session: &Session<T>,
    text: &'static str,
    detail: &str,
    replaces: impl Fn(&'static str) -> bool,
) {
    {
        let mut links = links().lock().unwrap();
        let link = links.entry(key(session)).or_default();
        if link.notice.is_some_and(|cur| !replaces(cur)) {
            return;
        }
        link.notice = Some(text);
    }
    log::warn!("ddpro: {text} ({detail})");
    push_mode(session, "fault", text, detail);
}

/// 两端专业协议版本不一致（谈判前看名片，或收到对端消息时发现）。
pub fn version_mismatch<T: InvokeUiSession>(session: &Session<T>, detail: &str) {
    fault(session, NOT_PRO_VERSION, detail);
}

/// 被控端拒绝了专业请求：按原因码换成用户看得懂的结论，原始说明放细节里。
pub fn rejected<T: InvokeUiSession>(session: &Session<T>, r: &Reject) {
    use reject::Reason;
    let text = match r.reason.enum_value() {
        Ok(Reason::DISPLAY_BUSY) => NOT_PRO_BUSY,
        Ok(Reason::VERSION_MISMATCH | Reason::MALFORMED | Reason::UNSUPPORTED_TIER) => NOT_PRO_VERSION,
        // 「主控端没有专业显示」，或格式谈不拢而本机就解不了 4:4:4
        Ok(Reason::DISPLAY_UNAVAILABLE) => NOT_PRO_UNSUPPORTED,
        Ok(Reason::NO_COMMON_FORMAT) if decode_caps().formats.is_empty() => NOT_PRO_UNSUPPORTED,
        _ => NOT_PRO_PEER_UNAVAILABLE,
    };
    fault(session, text, &r.detail);
}

/// 被控端报告它那一侧的专业画面出错、已退回普通画面：挂提示，关掉专业窗口（不等通道 3 秒没画面）。
pub fn peer_stopped<T: InvokeUiSession>(session: &Session<T>, s: &Stopped) {
    use stopped::Reason;
    let text = match s.reason.enum_value() {
        Ok(Reason::CHANNEL_LOST) => NOT_PRO_LOST,
        Ok(Reason::CHANNEL_FAILED) => NOT_PRO_CHANNEL_FAILED,
        Ok(Reason::NO_IMAGE) => NOT_PRO_NO_IMAGE,
        _ => NOT_PRO_PEER_FAILED,
    };
    // 被控端说的是确切原因：主控端等 3 秒没画面先挂上的通道原因（推测）让给它
    raise(session, text, &s.detail, |cur| cur == NOT_PRO_LOST || cur == NOT_PRO_CHANNEL_FAILED);
    // 丢掉通道，写线程随之关管道，专业窗口读到断开就自己退出（它报「会话让它关的」，不再改提示）
    if let Some(l) = links().lock().unwrap().get_mut(&key(session)) {
        l.down = None;
    }
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
        fault(session, NOT_PRO_DISPLAY, "cannot start the pro view: the remote display size is unknown");
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            fault(session, NOT_PRO_DISPLAY, &format!("cannot start the pro view: {e}"));
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
            fault(session, NOT_PRO_DISPLAY, &format!("cannot start the pro view: {e}"));
            return;
        }
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        fault(session, NOT_PRO_DISPLAY, "the pro view has no pipes");
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
        let mut ended: Option<ViewEnded> = None;
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
                Some(view_up::Union::Ready(_)) => {
                    // 专业画面出来了：这次请求成功，撤掉提示
                    links().lock().unwrap().entry(k).or_default().notice = None;
                    push_mode(&session, "", "", "");
                }
                Some(view_up::Union::Ended(e)) => {
                    log::info!("ddpro: the pro view is ending: {}", e.detail);
                    ended = Some(e);
                }
                _ => {}
            }
        };
        let status = child.wait();
        log::info!("ddpro: pro view closed ({reason}); exit status {status:?}");
        // 会话进程自己放掉的窗口（换屏、被控端报告出错、会话结束）不算；还挂着的这个窗口结束了才看原因
        let current = {
            let mut links = links().lock().unwrap();
            let l = links.entry(k).or_default();
            let current = l.down.as_ref().is_some_and(|(p, _)| *p == pid);
            if current {
                l.down = None;
            }
            current
        };
        if !current {
            return;
        }
        let detail = ended.as_ref().map_or(String::new(), |e| e.detail.clone());
        match ended.map(|e| e.kind.enum_value()) {
            Some(Ok(Kind::SESSION_CLOSED)) => {}
            Some(Ok(Kind::USER_CLOSED)) => {
                links().lock().unwrap().entry(k).or_default().notice = Some(NOT_PRO_USER_CLOSED);
                push_mode(&session, "user", NOT_PRO_USER_CLOSED, "");
            }
            Some(Ok(Kind::CHANNEL_LOST)) => fault(&session, NOT_PRO_LOST, &detail),
            Some(Ok(Kind::CHANNEL_FAILED)) => fault(&session, NOT_PRO_CHANNEL_FAILED, &detail),
            Some(_) => fault(&session, NOT_PRO_DISPLAY, &detail),
            // 没留下结束原因就退出了：多半是崩溃
            None => fault(
                &session,
                NOT_PRO_DISPLAY,
                &format!("the pro view exited unexpectedly ({reason}; exit status {status:?})"),
            ),
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
                let down = link.down.take();
                // 对新屏重新发起专业请求，算新的一次
                if down.is_some() {
                    link.notice = None;
                }
                return down.map(|_| s.display);
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
