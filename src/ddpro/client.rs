//! 主控端：看名片、发请求、读答复，同意后拉起专业窗口（Linux，见 `view_link`）。
//!
//! 能力如实报：Linux（Flutter 版）探测硬解和专业显示；其它平台还没有专业显示，报「未探测」，被控端不会开通道。
//! 诊断开关 `DDPRO_DIAG_RECEIVE_ONLY=1` 打开「只收不解」：解码 / 显示能力报成 4:4:4 8bit 可用（日志里写明是诊断），
//! 本进程自己收专业画面、记延迟，码流写进 `DDPRO_DIAG_DUMP` 指定的文件，用来在没有专业显示的机器上验证画面通道。

use crate::{
    client::{Data, Interface},
    ddpro::wire::{wrap, PEER_LOG_INTERVAL},
    ui_session_interface::{InvokeUiSession, Session},
};
use base::message_proto::{Message, PeerInfo};
use ddpro::{
    dd_pro,
    media::{Event, Ledger, Receiver},
    Chroma, DisplayCaps, FormatCaps, MediaChannel, VideoFormat,
};
use hbb_common::{log, throttled_log, tokio::sync::mpsc::UnboundedSender};
use serde_json::Value;
use std::{
    io::Write,
    time::{Duration, Instant},
};

const DIAG_RECEIVE_ONLY: &str = "DDPRO_DIAG_RECEIVE_ONLY";
const DIAG_DUMP: &str = "DDPRO_DIAG_DUMP";
/// 诊断用的故意丢包（千分比），验证丢包恢复
const DIAG_DROP: &str = "DDPRO_DIAG_DROP_PERMILLE";
/// 诊断用：每帧的分段延迟写成 CSV（微秒）
const DIAG_LATENCY: &str = "DDPRO_DIAG_LATENCY";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const STATS_INTERVAL: Duration = Duration::from_secs(5);

fn receive_only() -> bool {
    std::env::var(DIAG_RECEIVE_ONLY).is_ok_and(|v| v == "1")
}

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
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    super::view_link::clear_notice(session, display as i32);
    let session = session.clone();
    std::thread::spawn(move || {
        if let Some(m) = wrap(ddpro::hello(decode_caps(), display_caps(), display)) {
            session.send(Data::Message(m));
        }
    });
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

/// 开 / 关专业画面的抖动（默认不抖动；界面暂时没有入口，留给第 4 项高位深源压 8bit 时用）：记下来，专业窗口开着时
/// 马上告诉被控端；之后换屏开新的专业窗口也照这个设置（见 `view_link::start`）。
pub fn set_dither<T: InvokeUiSession>(_session: &Session<T>, _on: bool) {
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    for display in super::view_link::set_dither(_session, _on) {
        log::info!("ddpro: dither {} for remote display {display}", if _on { "on" } else { "off" });
        if let Some(m) = wrap(ddpro::set_dither(u32::try_from(display).unwrap_or(0), _on)) {
            _session.send(Data::Message(m));
        }
    }
}

/// RustDesk 每秒推质量面板数据之前（`io_loop` 的钩子）：专业窗口开着时，当前屏的帧率、编码、色度是通用画面那条链路的，
/// 换成专业画面的实测值。
pub fn quality_status<T: InvokeUiSession>(_session: &Session<T>, _status: &mut crate::client::QualityStatus) {
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    super::view_link::quality_status(_session, _status);
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

/// 处理被控端的答复。同意且带画面通道时拉起专业窗口（诊断「只收不解」时本进程自己收）；
/// `sender` 是 RustDesk 消息循环的发送端，它关了说明会话已结束。
pub fn on_message<T: InvokeUiSession>(data: &[u8], sender: &UnboundedSender<Data>, _session: &Session<T>) {
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
            if receive_only() {
                start_receiver(&accept.media, sender.clone());
            } else {
                #[cfg(all(feature = "flutter", target_os = "linux"))]
                super::view_link::start(&accept.media, sender.clone(), _session, accept.display_index as i32);
                #[cfg(not(all(feature = "flutter", target_os = "linux")))]
                log::error!("ddpro: this build has no pro view; the peer should not have accepted");
            }
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

fn start_receiver(media: &MediaChannel, sender: UnboundedSender<Data>) {
    let Some((peer, keys)) = ddpro::media_endpoint(media) else {
        log::error!("ddpro: the peer accepted but sent an unusable pro channel");
        return;
    };
    let drop = std::env::var(DIAG_DROP).ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    std::thread::spawn(move || {
        let mut r = match Receiver::connect(peer, keys, CONNECT_TIMEOUT, drop) {
            Ok(r) => r,
            Err(e) => {
                log::error!("ddpro: cannot open the pro channel to {peer}: {e}");
                return;
            }
        };
        log::info!("ddpro: pro channel open from {peer} (diagnostic drop {drop}‰)");
        let mut dump = diag_file(DIAG_DUMP);
        let mut csv = diag_file(DIAG_LATENCY);
        if let Some(f) = csv.as_mut() {
            writeln!(f, "rtp_ts,encode_us,send_us,net_us,spread_us,total_us").ok();
        }
        let mut ledger = Ledger::default();
        let mut last_stats = Instant::now();
        let mut loss_at: Option<Instant> = None;
        let end = loop {
            // 会话结束（RustDesk 的消息循环退出）就停
            if sender.is_closed() {
                break "session closed".to_owned();
            }
            match r.recv() {
                Ok(Event::Frame {
                    data,
                    timestamp,
                    times,
                    ..
                }) => {
                    if let Some(t) = loss_at.take() {
                        log::info!(
                            "ddpro: pro video recovered {:.1} ms after the loss",
                            t.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    // 第 5 步之前主控端只收不解，账只记到「末包到达」
                    let timed = times.and_then(|t| Some((t.transport_stages()?, t.last_arrival - t.captured_at()?)));
                    if let Some((stages, total)) = timed {
                        ledger.add_frame(&stages, total);
                        if let Some(f) = csv.as_mut() {
                            let [a, b, c, d] = stages.map(|(_, us)| us);
                            if writeln!(f, "{timestamp},{a},{b},{c},{d},{total}").is_err() {
                                csv = None;
                            }
                        }
                    }
                    if let Some(f) = dump.as_mut() {
                        if let Err(e) = f.write_all(&data) {
                            log::warn!("ddpro: stop dumping the pro video: {e}");
                            dump = None;
                        }
                    }
                }
                // 只收不解：无损方块不用
                Ok(Event::Tile { .. }) => {}
                Ok(Event::Loss { packets }) => {
                    loss_at.get_or_insert_with(Instant::now);
                    throttled_log!(
                        PEER_LOG_INTERVAL,
                        info,
                        "ddpro: pro video lost {packets} packet(s), keyframe requested on the pro channel"
                    );
                }
                Err(e) => break e.to_string(),
            }
            if last_stats.elapsed() >= STATS_INTERVAL {
                last_stats = Instant::now();
                log::info!("ddpro: pro video {:?}", r.stats);
                let clock = r.clock().map_or("no clock sync yet".to_owned(), |c| {
                    format!(
                        "clock rtt {:.2} ms offset {:.2} ms",
                        c.rtt_us as f64 / 1000.0,
                        c.offset_us as f64 / 1000.0
                    )
                });
                log::info!("ddpro: pro video latency {}; {clock}", ledger.take());
            }
        };
        log::info!("ddpro: pro channel closed ({end}); totals {:?}", r.stats);
    });
}

fn diag_file(var: &str) -> Option<std::fs::File> {
    let path = std::env::var(var).ok()?;
    std::fs::File::create(&path)
        .map_err(|e| log::warn!("ddpro: cannot create {var} file {path}: {e}"))
        .ok()
}

/// 能解 4:4:4 8bit（真探测到了，或诊断「只收不解」时照报）。
pub(super) fn format_444_8bit() -> FormatCaps {
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

/// 本机能解的格式：Linux 探测硬解，其它平台如实报「未探测」（诊断「只收不解」时除外）。
fn decode_caps() -> FormatCaps {
    if receive_only() {
        log::warn!("ddpro: {DIAG_RECEIVE_ONLY}=1: reporting 4:4:4 decode for a receive-only diagnostic, nothing will be displayed");
        return format_444_8bit();
    }
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    return super::view_link::decode_caps();
    #[cfg(not(all(feature = "flutter", target_os = "linux")))]
    FormatCaps::default()
}

/// 专业显示能不能用：Linux 探测 Wayland 和合成器，其它平台如实报「未探测」（诊断「只收不解」时除外）。
fn display_caps() -> DisplayCaps {
    if receive_only() {
        return DisplayCaps {
            checked: true,
            available: true,
            ..Default::default()
        };
    }
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
