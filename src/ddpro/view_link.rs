//! 主控端会话进程这一侧：拉起专业窗口进程（`--pro-view`），经它的标准输入 / 输出交换消息。
//!
//! 用子进程的管道而不是 RustDesk 的进程间通道：消息里有画面通道的密钥，子进程的管道只有父子两个进程碰得到；
//! 会话结束时这边关掉管道，专业窗口读到断开自己退出（P0 实施记录第 5 步第 4 项）。
//! 专业窗口交回的键鼠走 RustDesk 原有的输入入口（和 Flutter 窗口同一个入口、同一种键码）。
//!
//! 「非专业模式」提示（不静默降级，P0 实施记录「P0 收尾：模式提示」）也在这里：对端支持专业模式、但这次会话
//! 现在不在专业模式时，往通用窗口推 `ddpro_mode` 事件，界面在画面左上角挂常驻标签。

use crate::{
    client::{Data, Interface},
    ddpro::wire::{wrap, PEER_LOG_INTERVAL},
    ui_session_interface::{InvokeUiSession, Session},
};
use base::message_proto::{message, misc, KeyboardMode, Message, PeerInfo};
use ddpro::{
    reject, stopped, view_action, view_down, view_ended::Kind, view_up, DisplayCaps, FormatCaps,
    MediaChannel, Reject, Stopped, ViewDown, ViewEnded, ViewStart, ViewStats, ViewUp,
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

/// 每个会话（按 RustDesk 的会话号）：远程当前屏的编号、各块屏的位置尺寸，专业窗口开着时还有给它发消息的通道。
#[derive(Default)]
struct Link {
    index: i32,
    /// 远程各块屏的位置尺寸：对端信息里的显示列表，之后被控端每次报显示变化就更新那一块。
    /// 用户切屏时新屏的尺寸当场就有，不必等被控端回显示变化
    rects: HashMap<i32, (i32, i32, u32, u32)>,
    /// 专业窗口的进程号和给它发消息的通道。换屏时旧窗口还没退出新窗口就开了，旧窗口收尾时凭进程号
    /// 只清自己的
    down: Option<(u32, mpsc::Sender<ViewDown>)>,
    /// 这次专业请求（开会话、换屏各算一次）挂着的提示：只报第一个原因，后面的多半是它的连带结果；
    /// 例外是主控端因「没画面」推测的通道原因，被控端随后报来的确切原因可以替换它
    notice: Option<Notice>,
    /// 开着的专业窗口最近一次报的实测值（质量面板用）
    stats: Option<ViewStats>,
    /// 要被控端加抖动（默认不加）：本会话之后开的专业窗口都照这个设置
    dither: bool,
    /// 悬浮球的网络状态块开着：换屏开新的专业窗口时照旧
    show_status: bool,
}

static LINKS: OnceLock<Mutex<HashMap<u64, Link>>> = OnceLock::new();

fn links() -> &'static Mutex<HashMap<u64, Link>> {
    LINKS.get_or_init(Default::default)
}

fn key<T: InvokeUiSession>(session: &Session<T>) -> u64 {
    session.lc.read().unwrap().session_id
}

/// 「非专业模式」提示的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notice {
    PeerFailed,
    NoImage,
    PrivacyMode,
    Lost,
    ChannelFailed,
    Display,
    NoDisplayInfo,
    Unsupported,
    PeerUnavailable,
    Busy,
    Version,
    /// 不是故障：用户在悬浮球菜单里切到了通用模式（只对这次会话）
    General,
}

impl Notice {
    /// 标签上的结论。英文本身是多语言表的键（`src/lang`），界面按它翻译
    fn text(self) -> &'static str {
        match self {
            Notice::General => "General mode",
            Notice::PeerFailed => "Not in pro mode: the controlled side failed to capture or encode",
            Notice::NoImage => "Not in pro mode: the controlled side's screen shows no picture (no monitor connected?)",
            Notice::PrivacyMode => "Not in pro mode: privacy mode is on at the controlled side",
            Notice::Lost => "Not in pro mode: the pro channel was interrupted",
            Notice::ChannelFailed => "Not in pro mode: the pro channel could not be opened",
            Notice::Display => "Not in pro mode: this computer cannot show the pro view",
            Notice::NoDisplayInfo => "Not in pro mode: the size of the remote display is unknown",
            Notice::Unsupported => "Not in pro mode: this computer does not support pro mode",
            Notice::PeerUnavailable => "Not in pro mode: the controlled side cannot provide pro video",
            Notice::Busy => "Not in pro mode: this display is already in use",
            Notice::Version => "Not in pro mode: the pro protocol versions do not match",
        }
    }

    /// 配的说明：这意味着什么、该怎么处理（鼠标停在标签上显示，原始错误跟在后面）。同样是多语言表的键
    fn tip(self) -> &'static str {
        match self {
            Notice::General => "You switched to general mode for this session: the remote screen is shown here with the usual RustDesk video. Click Enter pro mode to switch back.",
            Notice::PeerFailed => "The controlled side could not capture the screen or encode the pro video. Reconnect to try again; if it keeps happening, check the controlled side's graphics driver.",
            Notice::NoImage => "The controlled side has no monitor connected, or its monitor is asleep, so the graphics card produces no picture. Connect a monitor (or an HDMI dummy plug), or enable a virtual display on the controlled side, then reconnect.",
            Notice::PrivacyMode => "On this version of Windows, the controlled side's privacy mode hides the screen from pro capture. Turn privacy mode off, then reconnect.",
            Notice::Lost => "The pro video stopped: either no video arrived here for 3 seconds, or the controlled side stopped hearing from this computer (for example, a firewall blocks UDP in one direction). Reconnect after checking the network or firewall.",
            Notice::ChannelFailed => "The pro video channel could not be set up. A firewall between the two computers may be blocking UDP. Check the network or firewall, then reconnect.",
            Notice::Display => "This computer failed to show the pro video (hardware decoding or display error). Reconnect to try again; if it keeps happening, check this computer's graphics driver and desktop.",
            Notice::NoDisplayInfo => "The controlled side did not report the size of this display. Reconnect to try again.",
            Notice::Unsupported => "This computer does not meet the pro mode requirements: hardware HEVC 4:4:4 decoding and a Wayland desktop.",
            Notice::PeerUnavailable => "The controlled side cannot provide pro video, for example because its graphics card cannot encode HEVC 4:4:4.",
            Notice::Busy => "Another connection is using this display of the controlled side (pro mode allows one connection per display). Reconnect after it ends, or switch to another display.",
            Notice::Version => "The two computers run different versions of the pro protocol. Upgrade the older side so both run the same version.",
        }
    }

    /// 主控端自己推测的通道原因（等 3 秒没画面）：被控端随后报来的确切原因可以替换它
    fn is_guess(self) -> bool {
        matches!(self, Notice::Lost | Notice::ChannelFailed)
    }

    /// 值得再请求一次（用户切到别的屏、或点「重新进入专业模式」）：和这块屏、这次通道有关的故障。
    /// 本机不支持、版本不一致，再请求也一样
    fn retryable(self) -> bool {
        !matches!(self, Notice::Unsupported | Notice::Version)
    }
}

/// 专业会话期间屏幕上只有一种窗口（P1 第 2 项）：专业画面出来后隐藏通用窗口（它所在的进程照常承载会话），
/// 专业模式出故障时显示回来，用户关掉专业画面就结束这次远程。`action` 为 `hide` / `show` / `close`。
/// 诊断开关 `DDPRO_DIAG_KEEP_WINDOW=1` 不隐藏，两个窗口并排对比用
fn push_window<T: InvokeUiSession>(session: &Session<T>, action: &str) {
    if action == "hide" && std::env::var("DDPRO_DIAG_KEEP_WINDOW").is_ok_and(|v| v == "1") {
        log::info!("ddpro: general window kept (DDPRO_DIAG_KEEP_WINDOW)");
        return;
    }
    log::info!("ddpro: general window: {action}");
    let conn_type = session.lc.read().unwrap().conn_type;
    if let Some(s) = crate::flutter::sessions::get_session_by_peer_id(session.get_id(), conn_type) {
        s.push_event("ddpro_window", &[("action", action)], &[]);
    }
}

/// 推给通用窗口：`level` 为 `fault`（红）、`general`（灰，用户自己切到通用模式），空串表示撤掉提示
fn push_mode<T: InvokeUiSession>(session: &Session<T>, level: &str, notice: Option<Notice>, detail: &str) {
    let (text, tip) = notice.map_or(("", ""), |n| (n.text(), n.tip()));
    let conn_type = session.lc.read().unwrap().conn_type;
    if let Some(s) = crate::flutter::sessions::get_session_by_peer_id(session.get_id(), conn_type) {
        s.push_event(
            "ddpro_mode",
            &[("level", level), ("text", text), ("tip", tip), ("detail", detail)],
            &[],
        );
    }
}

/// 记下要不要抖动；专业窗口开着时返回它的屏号，由调用方马上告诉被控端。
pub fn set_dither<T: InvokeUiSession>(session: &Session<T>, on: bool) -> Option<i32> {
    let mut links = links().lock().unwrap();
    let l = links.entry(key(session)).or_default();
    l.dither = on;
    l.down.is_some().then_some(l.index)
}

/// 质量面板里 RustDesk 原来没有的几项（专业画面的延迟、收到的码率、位深、丢包与补发、显示链路检查），
/// 推给 Dart 加在面板末尾；`None` 时撤掉这几行（专业窗口关了）。
fn push_quality<T: InvokeUiSession>(session: &Session<T>, stats: Option<&ViewStats>) {
    let conn_type = session.lc.read().unwrap().conn_type;
    let Some(s) = crate::flutter::sessions::get_session_by_peer_id(session.get_id(), conn_type) else {
        return;
    };
    let fields = stats.map(|v| {
        [
            ("latency_ms", v.latency_ms.to_string()),
            ("kbps", v.kbps.to_string()),
            ("bit_depth", v.bit_depth.to_string()),
            ("lost", v.lost_packets.to_string()),
            ("recovered", v.recovered_packets.to_string()),
            ("display_checked", v.display_checked.to_string()),
            (
                "display_issues",
                serde_json::to_string(&v.display_issues).unwrap_or_default(),
            ),
        ]
    });
    let fields: Vec<(&str, &str)> = fields
        .iter()
        .flatten()
        .map(|(k, v)| (*k, v.as_str()))
        .collect();
    s.push_event("ddpro_quality", &fields, &[]);
}

/// RustDesk 每秒推质量面板数据之前：专业窗口开着时，当前屏的帧率、编码、色度换成专业画面的实测值
/// （延迟、目标码率是 RustDesk 连接本身的，照旧）。
pub fn quality_status<T: InvokeUiSession>(session: &Session<T>, status: &mut crate::client::QualityStatus) {
    let links = links().lock().unwrap();
    let Some(l) = links.get(&key(session)) else {
        return;
    };
    let (Some(_), Some(v)) = (&l.down, &l.stats) else {
        return;
    };
    status.fps.insert(l.index.max(0) as usize, v.fps as i32);
    status.codec_format = Some(scrap::CodecFormat::H265);
    if !v.chroma.is_empty() {
        status.chroma = Some(v.chroma.clone());
    }
}

/// 发起一次专业请求（开会话、换屏），或对端根本不提供专业模式：撤掉上次挂的提示，这次请求里的第一个故障才挂新的。
pub fn clear_notice<T: InvokeUiSession>(session: &Session<T>) {
    let had = links().lock().unwrap().entry(key(session)).or_default().notice.take().is_some();
    if had {
        push_mode(session, "", None, "");
    }
}

/// 这次专业请求失败：写日志、挂红色提示（同一次请求只报第一个原因）。
fn fault<T: InvokeUiSession>(session: &Session<T>, notice: Notice, detail: &str) {
    raise(session, notice, detail, |_| false);
}

/// 挂红色提示；这次请求已经挂着提示时，只有 `replaces` 认可的才替换。
fn raise<T: InvokeUiSession>(session: &Session<T>, notice: Notice, detail: &str, replaces: impl Fn(Notice) -> bool) {
    {
        let mut links = links().lock().unwrap();
        let link = links.entry(key(session)).or_default();
        if link.notice.is_some_and(|cur| !replaces(cur)) {
            return;
        }
        link.notice = Some(notice);
    }
    log::warn!("ddpro: {} ({detail})", notice.text());
    push_mode(session, "fault", Some(notice), detail);
    // 不在专业模式了：通用窗口必须看得见（专业画面出来前出的故障，窗口本来就开着）
    push_window(session, "show");
}

/// 用户在悬浮球菜单里切到通用模式：丢掉通道（写线程随之关管道，专业窗口读到断开就告诉被控端不要了、自己退出，
/// 被控端退回通用画面——和切屏关旧窗口同一条路），挂灰色标签、显示通用窗口。
fn choose_general<T: InvokeUiSession>(session: &Session<T>) {
    {
        let mut links = links().lock().unwrap();
        let link = links.entry(key(session)).or_default();
        link.down = None;
        link.notice = Some(Notice::General);
    }
    push_mode(session, "general", Some(Notice::General), "");
    push_window(session, "show");
}

/// 用户点提示上的「重新进入专业模式」（故障）或「进入专业模式」（通用模式）：挂着的是值得再试的故障或通用模式时撤掉提示，
/// 返回当前屏号，由调用方重新请求。
pub fn retry<T: InvokeUiSession>(session: &Session<T>) -> Option<i32> {
    let index = {
        let mut links = links().lock().unwrap();
        let link = links.entry(key(session)).or_default();
        if link.down.is_some() || !link.notice.is_some_and(Notice::retryable) {
            return None;
        }
        link.notice = None;
        link.index
    };
    push_mode(session, "", None, "");
    Some(index)
}

/// 两端专业协议版本不一致（谈判前看名片，或收到对端消息时发现）。
pub fn version_mismatch<T: InvokeUiSession>(session: &Session<T>, detail: &str) {
    fault(session, Notice::Version, detail);
}

/// 被控端拒绝了专业请求：按原因码换成用户看得懂的结论，原始说明放细节里。
pub fn rejected<T: InvokeUiSession>(session: &Session<T>, r: &Reject) {
    use reject::Reason;
    let notice = match r.reason.enum_value() {
        Ok(Reason::DISPLAY_BUSY) => Notice::Busy,
        Ok(Reason::VERSION_MISMATCH | Reason::MALFORMED | Reason::UNSUPPORTED_TIER) => Notice::Version,
        // 「主控端没有专业显示」，或格式谈不拢而本机就解不了 4:4:4
        Ok(Reason::DISPLAY_UNAVAILABLE) => Notice::Unsupported,
        Ok(Reason::NO_COMMON_FORMAT) if decode_caps().formats.is_empty() => Notice::Unsupported,
        _ => Notice::PeerUnavailable,
    };
    fault(session, notice, &r.detail);
}

/// 被控端报告它那一侧的专业画面出错、已退回通用画面：挂提示，关掉专业窗口（不等通道 3 秒没画面）。
/// 说的不是当前屏（主控端切过屏，旧屏晚到的消息）就只记日志，不动新屏的专业窗口和提示。
pub fn peer_stopped<T: InvokeUiSession>(session: &Session<T>, s: &Stopped) {
    use stopped::Reason;
    let current = links().lock().unwrap().get(&key(session)).map(|l| l.index);
    if current != i32::try_from(s.display_index).ok() {
        log::info!(
            "ddpro: ignoring the stop of pro video on display {} (now on display {current:?}): {}",
            s.display_index,
            s.detail
        );
        return;
    }
    let notice = match s.reason.enum_value() {
        Ok(Reason::CHANNEL_LOST) => Notice::Lost,
        Ok(Reason::CHANNEL_FAILED) => Notice::ChannelFailed,
        Ok(Reason::NO_IMAGE) => Notice::NoImage,
        Ok(Reason::PRIVACY_MODE) => Notice::PrivacyMode,
        _ => Notice::PeerFailed,
    };
    // 被控端说的是确切原因：主控端等 3 秒没画面先挂上的通道原因（推测）让给它
    raise(session, notice, &s.detail, Notice::is_guess);
    // 丢掉通道，写线程随之关管道，专业窗口读到断开就自己退出（它报「会话让它关的」，不再改提示）
    if let Some(l) = links().lock().unwrap().get_mut(&key(session)) {
        l.down = None;
    }
}

/// 收到对端信息时记下远程当前屏的编号和各块屏的位置尺寸（专业窗口换算鼠标坐标要用）。
pub fn remember_display<T: InvokeUiSession>(pi: &PeerInfo, session: &Session<T>) {
    let mut links = links().lock().unwrap();
    let link = links.entry(key(session)).or_default();
    link.index = pi.current_display;
    link.rects = (0..)
        .zip(&pi.displays)
        .map(|(i, d)| (i, (d.x, d.y, d.width.max(0) as u32, d.height.max(0) as u32)))
        .collect();
}

/// 用户在 RustDesk 里切屏：换到别的屏、而专业窗口开着（或上次因和屏有关的故障没进专业模式）时关掉专业窗口，
/// 返回新屏的编号，由调用方对新屏重新发起专业请求。用户切到了通用模式的，留在通用模式。
pub fn on_switch_display<T: InvokeUiSession>(session: &Session<T>, display: i32) -> Option<i32> {
    let mut links = links().lock().unwrap();
    let link = links.entry(key(session)).or_default();
    if link.index == display {
        return None;
    }
    link.index = display;
    // 丢掉通道，写线程随之关管道，专业窗口读到断开就告诉被控端不要了、自己退出
    let had_view = link.down.take().is_some();
    (had_view || link.notice.is_some_and(|n| n.retryable() && n != Notice::General)).then_some(display)
}

/// 本机能不能硬解 4:4:4 并在显卡上转 RGB（只探测一次）。
pub fn decode_caps() -> FormatCaps {
    static CAPS: OnceLock<FormatCaps> = OnceLock::new();
    CAPS.get_or_init(|| {
        let probe = ddpro::dec::Device::open().and_then(|d| d.probe());
        match probe {
            Ok(c) if c.hevc_444_8bit && c.video_proc => {
                log::info!("ddpro: this machine can decode HEVC 4:4:4 8bit in hardware ({c:?})");
                super::client::format_444_8bit()
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
    let (display, display_count, display_index, show_status) = links()
        .lock()
        .unwrap()
        .get(&k)
        .map(|l| (l.rects.get(&l.index).copied().unwrap_or_default(), l.rects.len() as u32, l.index, l.show_status))
        .unwrap_or_default();
    if display.2 == 0 || display.3 == 0 {
        fault(session, Notice::NoDisplayInfo, "cannot start the pro view: the remote display size is unknown");
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            fault(session, Notice::Display, &format!("cannot start the pro view: {e}"));
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
            fault(session, Notice::Display, &format!("cannot start the pro view: {e}"));
            return;
        }
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        fault(session, Notice::Display, "the pro view has no pipes");
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
        display_count,
        display_index,
        show_status,
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
    let dither = {
        let mut links = links().lock().unwrap();
        let l = links.entry(k).or_default();
        l.down = Some((pid, tx));
        l.stats = None;
        // 诊断：界面还没有入口，用它从「加抖动」开始，验证「主控端 → 被控端改抖动」这条路
        if std::env::var("DDPRO_DIAG_DITHER").is_ok_and(|v| v == "1") {
            l.dither = true;
        }
        l.dither.then_some(l.index)
    };
    // 本会话要过抖动：新的专业会话（换屏后）也加。被控端谈成时已经登记好会话，这时发得上
    if let Some(index) = dither {
        if let Some(m) = wrap(ddpro::set_dither(u32::try_from(index).unwrap_or(0), true)) {
            session.send(Data::Message(m));
        }
    }

    // 写：启动参数、光标；会话结束或专业窗口结束时停，关掉管道
    std::thread::spawn(move || loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(m) => match ddpro::write_frame(&mut stdin, &m) {
                Ok(()) => {}
                // 超长的一条（比如超大的光标图案）跳过，管道里什么都没写，不用断开
                Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
                    throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: not forwarding a message to the pro view: {e}");
                }
                Err(e) => {
                    log::warn!("ddpro: cannot write to the pro view: {e}");
                    break;
                }
            },
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
                    // 会话号给键盘快捷键用（快捷键作用到哪个标签页）：专业窗口是本会话进程的子窗口，取本进程当前的标签页，
                    // 和 RustDesk 自己不经 Flutter 的全局键盘那条路（`keyboard::client::process_event`）一样
                    session.handle_flutter_raw_key_event(
                        crate::flutter::get_cur_session_id(),
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
                // 悬浮球菜单：调 RustDesk 会话已有的功能（和它自己工具栏里的同一套）
                Some(view_up::Union::Action(a)) => match a.union {
                    Some(view_action::Union::SwitchDisplay(i)) => {
                        log::info!("ddpro: floating menu: switch to remote display {i}");
                        session.switch_display(i);
                    }
                    Some(view_action::Union::CtrlAltDel(_)) => {
                        log::info!("ddpro: floating menu: Ctrl+Alt+Del");
                        session.ctrl_alt_del();
                    }
                    Some(view_action::Union::LockScreen(_)) => {
                        log::info!("ddpro: floating menu: lock the remote screen");
                        session.lock_screen();
                    }
                    Some(view_action::Union::ShowStatus(on)) => {
                        log::info!("ddpro: floating menu: network status {}", if on { "shown" } else { "hidden" });
                        links().lock().unwrap().entry(k).or_default().show_status = on;
                    }
                    Some(view_action::Union::GeneralMode(_)) => {
                        log::info!("ddpro: floating menu: switch to general mode");
                        choose_general(&session);
                    }
                    _ => {}
                },
                Some(view_up::Union::Ready(_)) => {
                    // 专业画面出来了：这次请求成功，撤掉提示；屏幕上只留专业画面
                    links().lock().unwrap().entry(k).or_default().notice = None;
                    push_mode(&session, "", None, "");
                    push_window(&session, "hide");
                }
                Some(view_up::Union::Ended(e)) => {
                    log::info!("ddpro: the pro view is ending: {}", e.detail);
                    ended = Some(e);
                }
                Some(view_up::Union::Stats(v)) => {
                    let current = {
                        let mut links = links().lock().unwrap();
                        let l = links.entry(k).or_default();
                        let current = l.down.as_ref().is_some_and(|(p, _)| *p == pid);
                        if current {
                            l.stats = Some(v.clone());
                        }
                        current
                    };
                    if current {
                        push_quality(&session, Some(&v));
                    }
                }
                _ => {}
            }
        };
        let status = child.wait();
        log::info!("ddpro: pro view closed ({reason}); exit status {status:?}");
        // 会话进程自己放掉的窗口（换屏、被控端报告出错、会话结束）不算；还挂着的这个窗口结束了才看原因
        let (current, none_open) = {
            let mut links = links().lock().unwrap();
            let l = links.entry(k).or_default();
            let current = l.down.as_ref().is_some_and(|(p, _)| *p == pid);
            if current {
                l.down = None;
            }
            if l.down.is_none() {
                l.stats = None;
            }
            (current, l.down.is_none())
        };
        // 被控端出错、切屏时窗口是先放掉再退出的（不再是当前窗口），面板那几行也要在这里撤；换屏后新窗口已经开着的不撤
        if none_open {
            push_quality(&session, None);
        }
        if !current {
            return;
        }
        let detail = ended.as_ref().map_or(String::new(), |e| e.detail.clone());
        match ended.map(|e| e.kind.enum_value()) {
            // 还挂着的窗口因为会话结束而关（RustDesk 连接断了）：RustDesk 的断开 / 重连提示在通用窗口里，要让人看得见
            Some(Ok(Kind::SESSION_CLOSED)) => push_window(&session, "show"),
            // 关专业画面就是结束这次远程（专业会话期间只有这一个窗口）
            Some(Ok(Kind::USER_CLOSED)) => push_window(&session, "close"),
            Some(Ok(Kind::CHANNEL_LOST)) => fault(&session, Notice::Lost, &detail),
            Some(Ok(Kind::CHANNEL_FAILED)) => fault(&session, Notice::ChannelFailed, &detail),
            Some(_) => fault(&session, Notice::Display, &detail),
            // 没留下结束原因就退出了：多半是崩溃
            None => fault(
                &session,
                Notice::Display,
                &format!("the pro view exited unexpectedly ({reason}; exit status {status:?})"),
            ),
        }
    });
}

/// RustDesk 连接里收到的消息里专业窗口要的（RustDesk 自己的处理照旧）：
/// - 光标：原样转给开着的专业窗口；
/// - 显示变化：记下那块屏的新位置尺寸；是当前屏的，原样转给专业窗口。被控端发这条消息不只在用户切屏时
///   （别的屏改了分辨率、主控端查询显示信息时也发），所以不从它推断「用户切了屏」——切屏看 [`on_switch_display`]。
pub fn on_rustdesk_message<T: InvokeUiSession>(msg: &Message, session: &Session<T>) {
    let switch = match &msg.union {
        Some(message::Union::CursorData(_)) | Some(message::Union::CursorId(_)) => None,
        Some(message::Union::Misc(m)) => match &m.union {
            Some(misc::Union::SwitchDisplay(s)) => Some(s),
            _ => return,
        },
        _ => return,
    };
    let mut links = links().lock().unwrap();
    let Some(link) = links.get_mut(&key(session)) else {
        return;
    };
    if let Some(s) = switch {
        if s.width > 0 && s.height > 0 {
            link.rects.insert(s.display, (s.x, s.y, s.width as u32, s.height as u32));
        }
        if s.display != link.index {
            return;
        }
    }
    let Some((_, down)) = link.down.as_ref() else {
        return;
    };
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
}
