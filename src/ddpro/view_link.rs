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
    ddpro::wire::PEER_LOG_INTERVAL,
    ui_session_interface::{InvokeUiSession, Session},
};
use base::message_proto::{
    message, misc, KeyboardMode, Message, Misc, PeerInfo, Resolution, SupportedResolutions, SwitchDisplay,
};
use ddpro::{
    reject, stopped, view_action, view_down, view_ended::Kind, view_up, Chroma, DisplayCaps, FormatCaps,
    MediaChannel, Reject, Stopped, VideoFormat, ViewDown, ViewEnded, ViewPoint, ViewStart, ViewUp,
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

// 专业窗口交来的按键位置码（X 键码 = evdev + 8）：左右 Win、L
const X_SUPER_L: i32 = 133;
const X_SUPER_R: i32 = 134;
const X_L: i32 = 46;

/// 每个会话（按 RustDesk 的会话号）：主窗口当前看的远程屏、各块屏的位置尺寸，以及各块屏的专业窗口。
#[derive(Default)]
struct Link {
    /// 主窗口当前看的远程屏
    index: i32,
    /// 远程各块屏的位置尺寸：对端信息里的显示列表，之后被控端每次报显示变化就更新那一块。
    /// 用户切屏时新屏的尺寸当场就有，不必等被控端回显示变化
    rects: HashMap<i32, (i32, i32, u32, u32)>,
    /// 远程各块屏支持的分辨率和原始分辨率（悬浮球「分辨率」菜单用）：RustDesk 自己收到的那份（对端信息里只有当前屏的列表，
    /// 之后被控端每次报显示变化都带上那块屏的），和它工具栏「分辨率」菜单用的同一份
    resolutions: HashMap<i32, (Vec<(i32, i32)>, (i32, i32))>,
    /// 按远程屏号：主窗口看的那块（`index`），和「将我的所有显示器用于远程会话」开的子窗口各看的一块（P1 第 8 项）
    views: HashMap<i32, View>,
    /// 对端提供专业模式、版本也对得上：RustDesk 之后为别的屏开子窗口时，那块屏也请求专业模式
    offered: bool,
    /// 悬浮球的网络状态块开着：换屏开新的专业窗口时照旧
    show_status: bool,
    /// 悬浮球的麦克风开着（这次会话发起了语音通话）：换屏开新的专业窗口时照旧
    mic: bool,
}

/// 一块远程屏的专业窗口和挂在显示它的那个 RustDesk 窗口上的提示。
#[derive(Default)]
struct View {
    /// 显示这块屏的 RustDesk 子窗口（「将我的所有显示器」开的）；None 是主窗口
    window: Option<uuid::Uuid>,
    /// 专业窗口的进程号和给它发消息的通道。换屏时旧窗口还没退出新窗口就开了，旧窗口收尾时凭进程号
    /// 只清自己的
    down: Option<(u32, mpsc::Sender<ViewDown>)>,
    /// 这次专业请求（开会话、换屏各算一次）挂着的提示：只报第一个原因，后面的多半是它的连带结果；
    /// 例外是主控端因「没画面」推测的通道原因，被控端随后报来的确切原因可以替换它
    notice: Option<Notice>,
}

impl Link {
    /// 这块屏现在有 RustDesk 窗口在显示：主窗口当前的屏，或某个子窗口的屏
    fn shown(&self, display: i32) -> bool {
        display == self.index || self.views.get(&display).is_some_and(|v| v.window.is_some())
    }

    /// 开着「将我的所有显示器」开的子窗口（多屏）
    fn multi(&self) -> bool {
        self.views.values().any(|v| v.window.is_some())
    }
}

static LINKS: OnceLock<Mutex<HashMap<u64, Link>>> = OnceLock::new();

fn links() -> &'static Mutex<HashMap<u64, Link>> {
    LINKS.get_or_init(Default::default)
}

fn key<T: InvokeUiSession>(session: &Session<T>) -> u64 {
    session.lc.read().unwrap().session_id
}

/// 进程号为 `pid` 的专业窗口还是这块屏当前的那个
fn is_current_view(k: u64, display: i32, pid: u32) -> bool {
    links()
        .lock()
        .unwrap()
        .get(&k)
        .and_then(|l| l.views.get(&display))
        .is_some_and(|v| v.down.as_ref().is_some_and(|(p, _)| *p == pid))
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
/// 多屏时（P1 第 8 项）只动显示 `display` 的那个窗口；`None` 是这条连接的所有窗口（关闭、切通用模式）。
fn push_window<T: InvokeUiSession>(session: &Session<T>, display: Option<i32>, action: &str) {
    match display {
        Some(d) => log::info!("ddpro: general window of remote display {d}: {action}"),
        None => log::info!("ddpro: general windows: {action}"),
    }
    push(session, display, "ddpro_window", &[("action", action)]);
}

/// 把事件推给显示 `display` 的 RustDesk 窗口：子窗口只推给它；主窗口那块推给除子窗口外的（就是主窗口自己）；
/// `None` 推给这条连接的所有窗口。
fn push<T: InvokeUiSession>(session: &Session<T>, display: Option<i32>, name: &str, fields: &[(&str, &str)]) {
    let conn_type = session.lc.read().unwrap().conn_type;
    let Some(s) = crate::flutter::sessions::get_session_by_peer_id(session.get_id(), conn_type) else {
        return;
    };
    let (window, children) = {
        let links = links().lock().unwrap();
        let link = links.get(&key(session));
        let window = display.and_then(|d| link?.views.get(&d)?.window);
        let children: Vec<uuid::Uuid> = link.map_or(Vec::new(), |l| l.views.values().filter_map(|v| v.window).collect());
        (window, children)
    };
    match (display, window) {
        (None, _) => s.push_event(name, fields, &[]),
        (Some(_), Some(w)) => s.push_event_to(name, fields, &[&w]),
        (Some(_), None) => s.push_event(name, fields, &children.iter().collect::<Vec<_>>()),
    }
}

/// 推给显示 `display` 的窗口：`level` 为 `fault`（红）、`general`（灰，用户自己切到通用模式），空串表示撤掉提示
fn push_mode<T: InvokeUiSession>(session: &Session<T>, display: Option<i32>, level: &str, notice: Option<Notice>, detail: &str) {
    let (text, tip) = notice.map_or(("", ""), |n| (n.text(), n.tip()));
    push(
        session,
        display,
        "ddpro_mode",
        &[("level", level), ("text", text), ("tip", tip), ("detail", detail)],
    );
}

/// 对端提供专业模式、版本也对得上（之后 RustDesk 为别的屏开子窗口时，那块屏也请求专业模式）。
pub fn mark_offered<T: InvokeUiSession>(session: &Session<T>) {
    links().lock().unwrap().entry(key(session)).or_default().offered = true;
}

/// 子窗口（「将我的所有显示器」开的）看的、现在没有专业窗口的屏：断线重连后要重新请求。
pub fn child_displays<T: InvokeUiSession>(session: &Session<T>) -> Vec<i32> {
    links().lock().unwrap().get(&key(session)).map_or(Vec::new(), |l| {
        l.views
            .iter()
            .filter(|(d, v)| v.window.is_some() && v.down.is_none() && **d != l.index)
            .map(|(d, _)| *d)
            .collect()
    })
}

/// 对某块屏发起一次专业请求（开会话、换屏、子窗口开出来）：撤掉上次挂的提示，这次请求里的第一个故障才挂新的。
pub fn clear_notice<T: InvokeUiSession>(session: &Session<T>, display: i32) {
    let had = links()
        .lock()
        .unwrap()
        .get_mut(&key(session))
        .and_then(|l| l.views.get_mut(&display)?.notice.take())
        .is_some();
    if had {
        push_mode(session, Some(display), "", None, "");
    }
}

/// 对端根本不提供专业模式（重连到了别的对端）：撤掉所有屏上次挂的提示。
pub fn clear_notices<T: InvokeUiSession>(session: &Session<T>) {
    let had: Vec<i32> = links()
        .lock()
        .unwrap()
        .get_mut(&key(session))
        .map_or(Vec::new(), |l| {
            l.views.iter_mut().filter_map(|(d, v)| v.notice.take().map(|_| *d)).collect()
        });
    for display in had {
        push_mode(session, Some(display), "", None, "");
    }
}

/// 这块屏的专业请求失败：写日志、挂红色提示（同一次请求只报第一个原因）。
fn fault<T: InvokeUiSession>(session: &Session<T>, display: i32, notice: Notice, detail: &str) {
    raise(session, display, notice, detail, |_| false);
}

/// 给这块屏挂红色提示；这次请求已经挂着提示时，只有 `replaces` 认可的才替换。
fn raise<T: InvokeUiSession>(
    session: &Session<T>,
    display: i32,
    notice: Notice,
    detail: &str,
    replaces: impl Fn(Notice) -> bool,
) {
    {
        let mut links = links().lock().unwrap();
        let view = links.entry(key(session)).or_default().views.entry(display).or_default();
        if view.notice.is_some_and(|cur| !replaces(cur)) {
            return;
        }
        view.notice = Some(notice);
    }
    log::warn!("ddpro: {} on remote display {display} ({detail})", notice.text());
    push_mode(session, Some(display), "fault", Some(notice), detail);
    // 不在专业模式了：通用窗口必须看得见（专业画面出来前出的故障，窗口本来就开着）
    push_window(session, Some(display), "show");
}

/// 用户在悬浮球菜单里切到通用模式（整个会话，多屏时各块屏一起）：丢掉通道（写线程随之关管道，专业窗口读到断开就告诉
/// 被控端不要了、自己退出，被控端退回通用画面——和切屏关旧窗口同一条路），挂灰色标签、显示通用窗口。
fn choose_general<T: InvokeUiSession>(session: &Session<T>) {
    let displays: Vec<i32> = {
        let mut links = links().lock().unwrap();
        let link = links.entry(key(session)).or_default();
        let index = link.index;
        link.views.entry(index).or_default();
        link.views
            .iter_mut()
            .map(|(d, v)| {
                v.down = None;
                v.notice = Some(Notice::General);
                *d
            })
            .collect()
    };
    for display in displays {
        push_mode(session, Some(display), "general", Some(Notice::General), "");
    }
    push_window(session, None, "show");
}

/// 用户在某个窗口（`window`）的提示上点「重新进入专业模式」（故障）或「进入专业模式」（通用模式）：挂着的是值得再试的故障时
/// 撤掉这块屏的提示；是通用模式时撤掉所有切到通用模式的屏的提示。返回要重新请求的屏号。
pub fn retry<T: InvokeUiSession>(session: &Session<T>, window: uuid::Uuid) -> Vec<i32> {
    let displays: Vec<i32> = {
        let mut links = links().lock().unwrap();
        let link = links.entry(key(session)).or_default();
        let display = link
            .views
            .iter()
            .find(|(_, v)| v.window == Some(window))
            .map_or(link.index, |(d, _)| *d);
        let Some(view) = link.views.get(&display) else {
            return Vec::new();
        };
        if view.down.is_some() || !view.notice.is_some_and(Notice::retryable) {
            return Vec::new();
        }
        let general = view.notice == Some(Notice::General);
        let picked: Vec<i32> = link
            .views
            .iter()
            .filter(|(d, v)| {
                v.down.is_none() && if general { v.notice == Some(Notice::General) } else { **d == display }
            })
            .map(|(d, _)| *d)
            .collect();
        for d in &picked {
            if let Some(v) = link.views.get_mut(d) {
                v.notice = None;
            }
        }
        picked
    };
    for display in &displays {
        push_mode(session, Some(*display), "", None, "");
    }
    displays
}

/// 两端专业协议版本不一致（谈判前看名片，或收到对端消息时发现）：挂在主窗口那块屏上。
pub fn version_mismatch<T: InvokeUiSession>(session: &Session<T>, detail: &str) {
    let index = links().lock().unwrap().entry(key(session)).or_default().index;
    fault(session, index, Notice::Version, detail);
}

/// 被控端拒绝了某块屏的专业请求：按原因码换成用户看得懂的结论，原始说明放细节里。
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
    let display = r.display_index as i32;
    // 晚到的拒绝：主控端在这之间切走了这块屏，提示不该挂到现在显示别的屏的窗口上
    if !links().lock().unwrap().get(&key(session)).is_some_and(|l| l.shown(display)) {
        log::info!("ddpro: ignoring a refusal for remote display {display} (no window shows it now): {}", r.detail);
        return;
    }
    fault(session, display, notice, &r.detail);
}

/// 被控端报告它那一侧某块屏的专业画面出错、已退回通用画面：挂提示，关掉专业窗口（不等通道 3 秒没画面）。
/// 说的屏现在没有窗口在显示（主控端切过屏，旧屏晚到的消息）就只记日志，不动别的屏的专业窗口和提示。
pub fn peer_stopped<T: InvokeUiSession>(session: &Session<T>, s: &Stopped) {
    use stopped::Reason;
    let display = s.display_index as i32;
    let shown = links().lock().unwrap().get(&key(session)).is_some_and(|l| l.shown(display));
    if !shown {
        log::info!(
            "ddpro: ignoring the stop of pro video on display {display} (no window shows it now): {}",
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
    raise(session, display, notice, &s.detail, Notice::is_guess);
    // 丢掉通道，写线程随之关管道，专业窗口读到断开就自己退出（它报「会话让它关的」，不再改提示）
    if let Some(v) = links().lock().unwrap().get_mut(&key(session)).and_then(|l| l.views.get_mut(&display)) {
        v.down = None;
    }
    want_general(session, display);
}

/// 被控端告诉这块屏的专业会话已因主控端关窗而结束（协议 v9 的「放回」）。
pub fn released<T: InvokeUiSession>(session: &Session<T>, display: i32) {
    log::info!("ddpro: the peer released remote display {display} from pro video");
    want_general(session, display);
}

/// 多屏时（P1 第 8 项）某块屏退出了专业模式、而它的窗口还在：照 RustDesk 新窗口的做法向被控端要回这块屏的普通画面，
/// 窗口照常显示。被控端只在连接没在看别的屏时才自己订回来（那是单窗口切屏的情形），多屏时总有别的屏，所以由这边要。
/// 单窗口时不动，照旧由被控端订回。
fn want_general<T: InvokeUiSession>(session: &Session<T>, display: i32) {
    let wanted = links().lock().unwrap().get(&key(session)).is_some_and(|l| {
        l.multi() && l.shown(display) && l.views.get(&display).is_none_or(|v| v.down.is_none())
    });
    if wanted {
        log::info!("ddpro: asking for the general video of remote display {display} again");
        session.capture_displays(vec![display], vec![], vec![]);
        session.refresh_video(display);
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
    let list: Vec<_> = pi.resolutions.resolutions.iter().map(|r| (r.width, r.height)).collect();
    for (i, d) in (0..).zip(&pi.displays) {
        let original = (d.original_resolution.width, d.original_resolution.height);
        let entry = link.resolutions.entry(i).or_default();
        entry.1 = original;
        if i == pi.current_display {
            entry.0 = list.clone();
        }
    }
}

/// 用户在 RustDesk 里切屏：换到别的屏、而专业窗口开着（或上次因和屏有关的故障没进专业模式）时关掉专业窗口，
/// 返回新屏的编号，由调用方对新屏重新发起专业请求。用户切到了通用模式的，留在通用模式。
pub fn on_switch_display<T: InvokeUiSession>(session: &Session<T>, display: i32) -> Option<i32> {
    let mut links = links().lock().unwrap();
    let link = links.entry(key(session)).or_default();
    if link.index == display {
        return None;
    }
    // 多屏时这块屏归某个子窗口（悬浮球不给切到它，见 `view.rs`）：不动主窗口那块屏的专业窗口
    if link.views.get(&display).is_some_and(|v| v.window.is_some()) {
        log::info!("ddpro: remote display {display} is shown in another window; keeping the pro view of display {}", link.index);
        return None;
    }
    let old = link.views.remove(&link.index).unwrap_or_default();
    link.index = display;
    // 丢掉通道（`old` 在这里放掉），写线程随之关管道，专业窗口读到断开就告诉被控端不要了、自己退出。
    // 提示挂在主窗口上，跟着主窗口走：重新请求时由 `clear_notice` 撤掉
    let had_view = old.down.is_some();
    link.views.insert(display, View { notice: old.notice, ..Default::default() });
    (had_view || old.notice.is_some_and(|n| n.retryable() && n != Notice::General)).then_some(display)
}

/// RustDesk 为某块屏开了子窗口：只接「将我的所有显示器用于远程会话」开的（P1 第 8 项，别的多屏看法专业模式不接）、
/// 对端提供专业模式、这块屏还没有专业窗口时，记下这个窗口，返回屏号由调用方请求专业模式。
pub fn on_window_start<T: InvokeUiSession>(session: &Session<T>, window: uuid::Uuid, displays: &[i32]) -> Option<i32> {
    let &[display] = displays else {
        return None;
    };
    if session.get_use_all_my_displays_for_the_remote_session() != "Y" {
        return None;
    }
    let mut links = links().lock().unwrap();
    let link = links.get_mut(&key(session))?;
    if !link.offered || display == link.index || link.views.get(&display).is_some_and(|v| v.down.is_some()) {
        return None;
    }
    link.views.insert(display, View { window: Some(window), ..Default::default() });
    Some(display)
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

/// 被控端同意后为远程屏 `display_index` 拉起专业窗口。`rustdesk` 是 RustDesk 消息循环的发送端，它关了说明会话已结束。
pub fn start<T: InvokeUiSession>(
    media: &MediaChannel,
    rustdesk: UnboundedSender<Data>,
    session: &Session<T>,
    display_index: i32,
) {
    let k = key(session);
    let (shown, is_main, display, origins, show_status, mic) = links()
        .lock()
        .unwrap()
        .get(&k)
        .map(|l| {
            let mut origins: Vec<_> = l.rects.iter().map(|(i, r)| (*i, r.0, r.1)).collect();
            origins.sort_unstable();
            (
                l.shown(display_index),
                display_index == l.index,
                l.rects.get(&display_index).copied().unwrap_or_default(),
                origins,
                l.show_status,
                l.mic,
            )
        })
        .unwrap_or_default();
    // 同意晚到：主控端在这之间切走了这块屏（被控端等不到专业窗口打招呼，会自己结束这次专业会话）
    if !shown {
        log::info!("ddpro: not starting a pro view for remote display {display_index}: no window shows it now");
        return;
    }
    if display.2 == 0 || display.3 == 0 {
        fault(
            session,
            display_index,
            Notice::NoDisplayInfo,
            "cannot start the pro view: the remote display size is unknown",
        );
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            fault(session, display_index, Notice::Display, &format!("cannot start the pro view: {e}"));
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
            fault(session, display_index, Notice::Display, &format!("cannot start the pro view: {e}"));
            return;
        }
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        fault(session, display_index, Notice::Display, "the pro view has no pipes");
        child.kill().ok();
        return;
    };
    log::info!("ddpro: pro view started for remote display {display_index} (pid {})", child.id());

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
        display_index,
        show_status,
        mic,
        display_origins: origins
            .into_iter()
            .map(|(_, x, y)| ViewPoint {
                x,
                y,
                ..Default::default()
            })
            .collect(),
        all_displays: session.get_use_all_my_displays_for_the_remote_session() == "Y",
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
    // 悬浮球「分辨率」菜单的初始数据：拼一条和被控端报显示变化同样的消息（之后被控端真报的照常转发过去，走同一处）
    let res = links().lock().unwrap().get(&k).and_then(|l| l.resolutions.get(&display_index).cloned());
    if let Some((list, original)) = res {
        let resolution = |(width, height): (i32, i32)| Resolution {
            width,
            height,
            ..Default::default()
        };
        let s = SwitchDisplay {
            display: display_index,
            x: display.0,
            y: display.1,
            width: display.2 as i32,
            height: display.3 as i32,
            resolutions: Some(SupportedResolutions {
                resolutions: list.into_iter().map(resolution).collect(),
                ..Default::default()
            })
            .into(),
            original_resolution: Some(resolution(original)).into(),
            ..Default::default()
        };
        let mut misc = Misc::new();
        misc.set_switch_display(s);
        let mut m = Message::new();
        m.set_misc(misc);
        if let Ok(bytes) = m.write_to_bytes() {
            tx.send(down(view_down::Union::Rustdesk(bytes))).ok();
        }
    }
    let pid = child.id();
    {
        let mut links = links().lock().unwrap();
        let view = links.entry(k).or_default().views.entry(display_index).or_default();
        view.down = Some((pid, tx));
    }

    let session_over = rustdesk.clone();
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
        // 左右 Win 键按着没有（Win+L 用）
        let mut win_down = [false; 2];
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
                    // Windows 不认模拟出来的 Win+L：和 RustDesk 传统键盘模式一样（`keyboard.rs` 的 `legacy_keyboard_mode`），
                    // 远程是 Windows 时按 Win+L 就发 RustDesk 的「锁定远程」。L 照常发过去（Windows 忽略它，也免得单独松开
                    // Win 被当成打开开始菜单）
                    match k.position_code {
                        X_SUPER_L => win_down[0] = k.down,
                        X_SUPER_R => win_down[1] = k.down,
                        X_L if k.down && win_down.contains(&true) && session.peer_platform() == "Windows" => {
                            log::info!("ddpro: pro view Win+L: lock the remote screen");
                            session.lock_screen();
                        }
                        _ => {}
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
                // 换屏 / 被拒的时序下旧窗口晚到的「就绪」和菜单动作：只认当前这个专业窗口的（和实测值那一支一样核对进程号），
                // 否则会清掉新屏的提示、藏起通用窗口，或把动作作用到新屏上
                Some(view_up::Union::Action(_)) | Some(view_up::Union::Ready(_))
                    if !is_current_view(k, display_index, pid) =>
                {
                    log::info!("ddpro: ignored a late message from an old pro view (pid {pid})");
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
                    // 用 RustDesk 自己的语音通话：主控端采集麦克风发过去，专业会话的被控端自动接受、送进虚拟麦克风（P2 第 2 项）。
                    // 多屏时各窗口的开关各自显示，按「要开 / 要关」处理而不是翻转，点哪个都不会重复发起
                    Some(view_action::Union::Mic(on)) => {
                        log::info!("ddpro: floating menu: microphone {}", if on { "on" } else { "off" });
                        let was = std::mem::replace(&mut links().lock().unwrap().entry(k).or_default().mic, on);
                        if on && !was {
                            session.request_voice_call();
                        } else if !on && was {
                            session.close_voice_call();
                        }
                    }
                    Some(view_action::Union::GeneralMode(_)) => {
                        log::info!("ddpro: floating menu: switch to general mode");
                        choose_general(&session);
                    }
                    // 和 RustDesk 工具栏「分辨率」菜单同一个函数：被控端改分辨率、记下按屏的自定义分辨率、断开时恢复原分辨率
                    // 都照它来；专业画面随被控端的显示变化自己重建
                    Some(view_action::Union::ChangeResolution(r)) => {
                        log::info!("ddpro: floating menu: change display {display_index} to {}x{}", r.width, r.height);
                        session.change_resolution(display_index, r.width as i32, r.height as i32);
                    }
                    _ => {}
                },
                Some(view_up::Union::Ready(_)) => {
                    // 专业画面出来了：这次请求成功，撤掉提示；屏幕上只留专业画面（多屏时只藏显示这块屏的窗口）
                    if let Some(v) = links().lock().unwrap().get_mut(&k).and_then(|l| l.views.get_mut(&display_index)) {
                        v.notice = None;
                    }
                    push_mode(&session, Some(display_index), "", None, "");
                    push_window(&session, Some(display_index), "hide");                }
                Some(view_up::Union::Ended(e)) => {
                    log::info!("ddpro: the pro view is ending: {}", e.detail);
                    ended = Some(e);
                }
                _ => {}
            }
        };
        let status = child.wait();
        log::info!("ddpro: pro view of remote display {display_index} closed ({reason}); exit status {status:?}");
        // 会话进程自己放掉的窗口（换屏、被控端报告出错、会话结束）不算；还挂着的这个窗口结束了才看原因
        let (current, window_has_view) = {
            let mut links = links().lock().unwrap();
            let l = links.entry(k).or_default();
            let mut current = false;
            if let Some(v) = l.views.get_mut(&display_index) {
                current = v.down.as_ref().is_some_and(|(p, _)| *p == pid);
                if current {
                    v.down = None;
                }
            }
            // 显示这块屏的窗口现在有没有开着的专业窗口：这块屏还在原来的窗口里就看它；主窗口换了屏就看主窗口的新屏
            let window_has_view = match l.views.get(&display_index) {
                Some(v) => v.down.is_some(),
                None => l.views.get(&l.index).is_some_and(|v| v.down.is_some()),
            };
            (current, window_has_view)
        };
        if !current {
            // 换屏时会话进程先放掉主窗口的旧专业窗口、再给新屏发专业请求。这个空档里 RustDesk 连接断了的话新窗口不会再开，
            // 通用窗口却还藏着，断开 / 重连提示看不到（2026-10-20 审查第 4 条）。等到新窗口开起来或挂上了提示（那时通用窗口
            // 已经照常显示）就不管；等的时候连接断了，把通用窗口露出来
            if is_main && !window_has_view {
                while !session_over.is_closed() {
                    let settled = links().lock().unwrap().get(&k).is_none_or(|l| {
                        l.views.get(&l.index).is_some_and(|v| v.down.is_some() || v.notice.is_some())
                    });
                    if settled {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                log::info!("ddpro: the RustDesk connection ended while switching displays; showing the general windows");
                push_window(&session, None, "show");
            }
            return;
        }
        let detail = ended.as_ref().map_or(String::new(), |e| e.detail.clone());
        let d = display_index;
        match ended.map(|e| e.kind.enum_value()) {
            // 还挂着的窗口因为会话结束而关（RustDesk 连接断了）：RustDesk 的断开 / 重连提示在通用窗口里，要让人看得见
            Some(Ok(Kind::SESSION_CLOSED)) => push_window(&session, Some(d), "show"),
            // 关专业画面就是结束这次远程（多屏时几块屏是一个整体，关任何一个都结束）
            Some(Ok(Kind::USER_CLOSED)) => push_window(&session, None, "close"),
            Some(Ok(Kind::CHANNEL_LOST)) => fault(&session, d, Notice::Lost, &detail),
            Some(Ok(Kind::CHANNEL_FAILED)) => fault(&session, d, Notice::ChannelFailed, &detail),
            Some(_) => fault(&session, d, Notice::Display, &detail),
            // 没留下结束原因就退出了：多半是崩溃
            None => fault(
                &session,
                d,
                Notice::Display,
                &format!("the pro view exited unexpectedly ({reason}; exit status {status:?})"),
            ),
        }
    });
}

/// RustDesk 连接里收到的消息里专业窗口要的（RustDesk 自己的处理照旧）：
/// - 光标形状、被别的东西挪动后的光标位置：原样转给所有开着的专业窗口（位置在哪块屏上由各窗口自己判断）；
/// - 显示变化：记下那块屏的新位置尺寸；那块屏开着专业窗口的，原样转给它。被控端发这条消息不只在用户切屏时
///   （别的屏改了分辨率、主控端查询显示信息时也发），所以不从它推断「用户切了屏」——切屏看 [`on_switch_display`]。
pub fn on_rustdesk_message<T: InvokeUiSession>(msg: &Message, session: &Session<T>) {
    let switch = match &msg.union {
        Some(message::Union::CursorData(_))
        | Some(message::Union::CursorId(_))
        | Some(message::Union::CursorPosition(_)) => None,
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
        if !s.resolutions.resolutions.is_empty() {
            link.resolutions.insert(
                s.display,
                (
                    s.resolutions.resolutions.iter().map(|r| (r.width, r.height)).collect(),
                    (s.original_resolution.width, s.original_resolution.height),
                ),
            );
        }
    }
    let downs: Vec<_> = link
        .views
        .iter()
        .filter(|(d, _)| switch.is_none_or(|s| s.display == **d))
        .filter_map(|(_, v)| v.down.as_ref().map(|(_, down)| down))
        .collect();
    if downs.is_empty() {
        return;
    }
    match msg.write_to_bytes() {
        Ok(bytes) => {
            for down in downs {
                down.send(ViewDown {
                    union: Some(view_down::Union::Rustdesk(bytes.clone())),
                    ..Default::default()
                })
                .ok();
            }
        }
        Err(e) => throttled_log!(
            PEER_LOG_INTERVAL,
            warn,
            "ddpro: cannot forward a message to the pro view: {e}"
        ),
    }
}
