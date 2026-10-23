//! 主控端：看名片、发请求、读答复，同意后拉起专业窗口（Linux，见 `view_link`）。
//!
//! 能力如实报：Linux（Flutter 版）探测硬解和专业显示；其它平台还没有专业显示，报「未探测」，被控端不会开通道。

use crate::{
    client::{Data, Interface},
    ddpro::wire::{wrap, PEER_LOG_INTERVAL},
    ui_session_interface::{InvokeUiSession, Session},
};
use base::message_proto::{Message, PeerInfo};
use ddpro::{dd_pro, DisplayCaps, FormatCaps};
use hbb_common::{log, throttled_log, tokio::sync::mpsc::UnboundedSender};
use serde_json::Value;

/// 收到被控端的登录应答后决定要不要发起专业会话。
/// 只在远程桌面会话里发起；文件传输、终端、端口转发等会话不发。
pub fn on_peer_info<T: InvokeUiSession>(pi: &PeerInfo, session: &Session<T>) {
    if !session.is_default() {
        return;
    }
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    super::view_link::remember_display(pi, session);
    let card = serde_json::from_str::<Value>(&pi.platform_additions)
        .ok()
        .and_then(|v| v.get(ddpro::NAME_CARD_KEY).and_then(Value::as_u64));
    let Some(peer_version) = card else {
        log::info!("ddpro: the peer does not offer pro mode, staying in general mode");
        // 重连到了不提供专业模式的对端：上次挂的提示不再适用（对普通 RustDesk 对端不显示提示）
        #[cfg(all(feature = "flutter", target_os = "linux"))]
        super::view_link::clear_notices(session);
        return;
    };
    let peer_version = u32::try_from(peer_version).unwrap_or(u32::MAX);
    if let Some(mismatch) = ddpro::version_mismatch(peer_version) {
        log::error!("ddpro: not starting a pro session: {mismatch}");
        #[cfg(all(feature = "flutter", target_os = "linux"))]
        super::view_link::version_mismatch(session, &mismatch);
        return;
    }
    let display = u32::try_from(pi.current_display).unwrap_or(0);
    log::info!("ddpro: the peer offers pro protocol v{peer_version}, requesting a pro session for display {display}");
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    super::view_link::mark_offered(session);
    // USB 透传（数位板）两种模式都用，和专业画面谈不谈得成无关
    #[cfg(target_os = "linux")]
    super::usb_client::start(session);
    request_pro(session, display);
    // 断线重连：「将我的所有显示器」开的子窗口还在、不会再开一次，它们的屏也在这里重新请求（P1 第 8 项）
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    for d in super::view_link::child_displays(session) {
        log::info!("ddpro: requesting a pro session for remote display {d} shown in another window");
        request_pro(session, u32::try_from(d).unwrap_or(0));
    }
}

/// RustDesk 为某块远程屏开了新窗口（`session_start_with_displays` 的钩子；「将我的所有显示器用于远程会话」连上时按屏铺满就走这里）：
/// 对端提供专业模式时这块屏也进专业模式，专业窗口和这个窗口对应（P1 第 8 项）。别的多屏看法（每屏单独窗口、所有屏一个窗口）不接。
#[cfg(all(feature = "flutter", target_os = "linux"))]
pub fn on_window_start<T: InvokeUiSession>(session: &Session<T>, window: uuid::Uuid, displays: &[i32]) {
    if let Some(display) = super::view_link::on_window_start(session, window, displays) {
        log::info!("ddpro: RustDesk opened a window for remote display {display}; requesting a pro session for it");
        request_pro(session, u32::try_from(display).unwrap_or(0));
    }
}

/// 对 `display` 发起专业请求（开会话、换屏各算新的一次）。本机能力第一次探测要打开显卡、连 Wayland，
/// 几十到几百毫秒，放到单独的线程里做，不占 RustDesk 的消息循环；请求经会话的发送通道发出。
fn request_pro<T: InvokeUiSession>(session: &Session<T>, display: u32) {
    let session = session.clone();
    // `thread::spawn` 起不来会 panic，release 下整个会话进程直接退出：起不来就只记日志，这次不请求
    let spawned = std::thread::Builder::new().name("ddpro-request".into()).spawn(move || {
        // 撤提示也在这个线程里：工具栏切屏时调用方拿着会话窗口表的写锁（`flutter.rs` 的 `session_switch_display`），
        // 撤提示要往界面推事件、读同一张表，当场做会卡死。故障只在答复之后出现，先撤后发顺序不变
        #[cfg(all(feature = "flutter", target_os = "linux"))]
        super::view_link::clear_notice(&session, display as i32);
        if let Some(m) = wrap(ddpro::hello(decode_caps(), display_caps(), display)) {
            session.send(Data::Message(m));
        }
    });
    if let Err(e) = spawned {
        log::error!("ddpro: cannot start the thread that requests a pro session for remote display {display}: {e}");
    }
}

/// 用户点「非专业模式」提示上的「重新进入专业模式」，或通用模式标签上的「进入专业模式」（`_window` 是点的那个窗口）：
/// 故障值得再试、或用户切过通用模式时，对那个窗口的屏重新发起专业请求（切过通用模式的，各块屏一起回专业模式）。
pub fn retry<T: InvokeUiSession>(_session: &Session<T>, _window: uuid::Uuid) {
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    for display in super::view_link::retry(_session, _window) {
        log::info!("ddpro: retrying pro mode on remote display {display}");
        request_pro(_session, u32::try_from(display).unwrap_or(0));
    }
}

/// 用户在 RustDesk 里切屏（`Session::switch_display` 的钩子）：专业窗口开着、或上次因和屏有关的故障没进专业模式时，
/// 关掉旧屏的专业窗口、对新屏重新发起专业请求（「同屏不共享」照旧由被控端检查）。
pub fn on_switch_display<T: InvokeUiSession>(_session: &Session<T>, _display: i32) {
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    if let Some(display) = super::view_link::on_switch_display(_session, _display) {
        log::info!("ddpro: switched to remote display {display}; requesting a pro session for it");
        request_pro(_session, u32::try_from(display).unwrap_or(0));
    }
}

/// 处理被控端的答复。同意且带画面通道时拉起专业窗口；
/// `_sender` 是 RustDesk 消息循环的发送端，它关了说明会话已结束。
pub fn on_message<T: InvokeUiSession>(data: &[u8], _sender: &UnboundedSender<Data>, _session: &Session<T>) {
    let msg = match ddpro::decode(data) {
        Ok(msg) => msg,
        Err(err) => {
            throttled_log!(
                PEER_LOG_INTERVAL,
                warn,
                "ddpro: malformed pro protocol message from the peer: {err}"
            );
            return;
        }
    };
    if let Some(mismatch) = ddpro::version_mismatch(msg.version) {
        throttled_log!(
            PEER_LOG_INTERVAL,
            error,
            "ddpro: pro session failed: {mismatch}"
        );
        #[cfg(all(feature = "flutter", target_os = "linux"))]
        super::view_link::version_mismatch(_session, &mismatch);
        return;
    }
    match msg.union {
        Some(dd_pro::Union::Accept(accept)) if accept.format.is_some() => {
            throttled_log!(
                PEER_LOG_INTERVAL,
                info,
                "ddpro: pro session accepted, format {}",
                ddpro::describe_format(&accept.format)
            );
            #[cfg(all(feature = "flutter", target_os = "linux"))]
            super::view_link::start(
                &accept.media,
                &accept.format,
                accept.hdr_capable,
                _sender.clone(),
                _session,
                accept.display_index as i32,
            );
            #[cfg(not(all(feature = "flutter", target_os = "linux")))]
            log::error!("ddpro: this build has no pro view; the peer should not have accepted");
        }
        Some(dd_pro::Union::Accept(_)) => throttled_log!(
            PEER_LOG_INTERVAL,
            info,
            "ddpro: pro protocol agreed (v{}); capabilities not probed yet",
            ddpro::PROTOCOL_VERSION
        ),
        Some(dd_pro::Union::Reject(r)) => {
            throttled_log!(
                PEER_LOG_INTERVAL,
                error,
                "ddpro: pro session refused ({:?}): {}",
                r.reason,
                r.detail
            );
            #[cfg(all(feature = "flutter", target_os = "linux"))]
            super::view_link::rejected(_session, &r);
        }
        Some(dd_pro::Union::Stopped(s)) => {
            throttled_log!(
                PEER_LOG_INTERVAL,
                warn,
                "ddpro: the peer stopped pro video: {}",
                s.detail
            );
            #[cfg(all(feature = "flutter", target_os = "linux"))]
            super::view_link::peer_stopped(_session, &s);
        }
        Some(
            u @ (dd_pro::Union::UsbStatus(_)
            | dd_pro::Union::UsbOpen(_)
            | dd_pro::Union::UsbData(_)
            | dd_pro::Union::UsbClose(_)),
        ) => {
            #[cfg(target_os = "linux")]
            super::usb_client::on_message(_session, u);
            #[cfg(not(target_os = "linux"))]
            let _ = u;
        }
        Some(dd_pro::Union::HdrResult(r)) => {
            #[cfg(all(feature = "flutter", target_os = "linux"))]
            hdr_switched(_session, _sender.clone(), r);
            #[cfg(not(all(feature = "flutter", target_os = "linux")))]
            let _ = r;
        }
        Some(dd_pro::Union::Released(r)) => {
            #[cfg(all(feature = "flutter", target_os = "linux"))]
            super::view_link::released(_session, r.display_index as i32);
            #[cfg(not(all(feature = "flutter", target_os = "linux")))]
            let _ = r;
        }
        _ => throttled_log!(
            PEER_LOG_INTERVAL,
            warn,
            "ddpro: ignoring an unexpected pro protocol message from the peer"
        ),
    }
}

/// 被控端切完了 HDR（悬浮球「HDR」）：切成了就也切本机的屏；不论成没成都关掉专业窗口、等两边的显示稳下来再重新请求
/// （被控端可能切了一部分屏，旧的专业会话的格式已经不对）。切显示模式会阻塞，放在单独的线程里，不占 RustDesk 的消息循环
#[cfg(all(feature = "flutter", target_os = "linux"))]
fn hdr_switched<T: InvokeUiSession>(session: &Session<T>, rustdesk: UnboundedSender<Data>, r: ddpro::HdrResult) {
    if r.ok {
        log::info!("ddpro: hdr: the peer turned HDR {}", if r.on { "on" } else { "off" });
    } else {
        log::warn!("ddpro: hdr: the peer could not turn HDR {}: {}", if r.on { "on" } else { "off" }, r.detail);
    }
    let session = session.clone();
    let spawned = std::thread::Builder::new().name("ddpro-hdr".into()).spawn(move || {
        let displays = super::view_link::switch_hdr(&session, rustdesk, r.on, r.ok);
        // 两边切显示模式后桌面和被控端的采集都要重建一下，太早请求会按旧状态谈
        std::thread::sleep(std::time::Duration::from_millis(1500));
        super::view_link::end_hdr_switch(&session);
        for d in displays {
            log::info!("ddpro: hdr: requesting a pro session for remote display {d} again");
            request_pro(&session, u32::try_from(d).unwrap_or(0));
        }
    });
    if let Err(e) = spawned {
        log::error!("ddpro: hdr: cannot start the thread that switches HDR: {e}");
    }
}

/// 本机能解的格式：Linux 探测硬解，其它平台如实报「未探测」。
fn decode_caps() -> FormatCaps {
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    return super::view_link::decode_caps();
    #[cfg(not(all(feature = "flutter", target_os = "linux")))]
    FormatCaps::default()
}

/// 专业显示能不能用：Linux 探测 Wayland 和合成器，其它平台如实报「未探测」。
fn display_caps() -> DisplayCaps {
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    return super::view_link::display_caps();
    #[cfg(not(all(feature = "flutter", target_os = "linux")))]
    DisplayCaps::default()
}

/// RustDesk 连接里收到的每条消息都过一下（RustDesk 自己的处理照旧）：光标、当前屏的显示变化转给开着的专业窗口。
pub fn on_rustdesk_message<T: InvokeUiSession>(_msg: &Message, _session: &Session<T>) {
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    super::view_link::on_rustdesk_message(_msg, _session);
}
