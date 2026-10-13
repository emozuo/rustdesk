//! 主控端：看名片、发请求、读答复。

use super::PEER_LOG_INTERVAL;
use base::message_proto::{Message, PeerInfo};
use ddpro::{dd_pro, DisplayCaps, FormatCaps};
use hbb_common::{log, throttled_log};
use serde_json::Value;

/// 收到被控端的登录应答后决定要不要发起专业会话，返回要发出的请求。
/// 只在远程桌面会话里发起；文件传输、终端、端口转发等会话不发。
pub fn on_peer_info(pi: &PeerInfo, is_remote_desktop: bool) -> Option<Message> {
    if !is_remote_desktop {
        return None;
    }
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
    log::info!("ddpro: the peer offers pro protocol v{peer_version}, requesting a pro session");
    super::wrap(ddpro::hello(decode_caps(), display_caps()))
}

/// 处理被控端的答复。
pub fn on_message(data: &[u8]) {
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
        Some(dd_pro::Union::Accept(accept)) if accept.format.is_some() => throttled_log!(
            PEER_LOG_INTERVAL,
            info,
            "ddpro: pro session accepted, format {}",
            ddpro::describe_format(&accept.format)
        ),
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

/// 本机能解的格式。硬解探测在第 5 步接入，在那之前如实报「未探测」。
fn decode_caps() -> FormatCaps {
    FormatCaps::default()
}

/// 专业显示能不能用。原生窗口在第 5 步接入，在那之前如实报「未探测」。
fn display_caps() -> DisplayCaps {
    DisplayCaps::default()
}
