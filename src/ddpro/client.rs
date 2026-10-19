//! 主控端：看名片、发请求、读答复，同意后拉起专业窗口（Linux，见 `view_link`）。
//!
//! 能力如实报：Linux（Flutter 版）探测硬解和专业显示；其它平台还没有专业显示，报「未探测」，被控端不会开通道。
//! 诊断开关 `DDPRO_DIAG_RECEIVE_ONLY=1` 打开「只收不解」：解码 / 显示能力报成 4:4:4 8bit 可用（日志里写明是诊断），
//! 本进程自己收专业画面、记延迟，码流写进 `DDPRO_DIAG_DUMP` 指定的文件，用来在没有专业显示的机器上验证画面通道。

use super::wire::{wrap, PEER_LOG_INTERVAL};
use crate::{
    client::Data,
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

/// 收到被控端的登录应答后决定要不要发起专业会话，返回要发出的请求。
/// 只在远程桌面会话里发起；文件传输、终端、端口转发等会话不发。
pub fn on_peer_info<T: InvokeUiSession>(pi: &PeerInfo, is_remote_desktop: bool, _session: &Session<T>) -> Option<Message> {
    if !is_remote_desktop {
        return None;
    }
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    super::view_link::remember_display(pi, _session);
    let card = serde_json::from_str::<Value>(&pi.platform_additions)
        .ok()
        .and_then(|v| v.get(ddpro::NAME_CARD_KEY).and_then(Value::as_u64));
    let Some(peer_version) = card else {
        log::info!("ddpro: the peer does not offer pro mode, staying in general mode");
        return None;
    };
    let peer_version = u32::try_from(peer_version).unwrap_or(u32::MAX);
    if let Some(mismatch) = ddpro::version_mismatch(peer_version) {
        log::error!("ddpro: not starting a pro session: {mismatch}");
        return None;
    }
    let display = u32::try_from(pi.current_display).unwrap_or(0);
    log::info!("ddpro: the peer offers pro protocol v{peer_version}, requesting a pro session for display {display}");
    wrap(ddpro::hello(decode_caps(), display_caps(), display))
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
                super::view_link::start(&accept.media, sender.clone(), _session);
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
        Some(dd_pro::Union::Reject(r)) => throttled_log!(
            PEER_LOG_INTERVAL,
            error,
            "ddpro: pro session refused ({:?}): {}",
            r.reason,
            r.detail
        ),
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

fn format_444_8bit() -> FormatCaps {
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

/// RustDesk 连接里收到的每条消息都过一下（RustDesk 自己的处理照旧）：光标、显示变化转给开着的专业窗口；
/// 用户换到了别的屏时，专业窗口已经关掉，这里对新屏重新发起专业请求（「同屏不共享」照旧由被控端检查）。
pub fn on_rustdesk_message<T: InvokeUiSession>(_msg: &Message, _session: &Session<T>) {
    #[cfg(all(feature = "flutter", target_os = "linux"))]
    if let Some(display) = super::view_link::on_rustdesk_message(_msg, _session) {
        log::info!("ddpro: the remote display switched to {display}; requesting a pro session for it");
        let display = u32::try_from(display).unwrap_or(0);
        if let Some(m) = wrap(ddpro::hello(decode_caps(), display_caps(), display)) {
            use crate::client::Interface;
            _session.send(Data::Message(m));
        }
    }
}
