//! 被控端：递名片、答复主控端的请求。

use super::wire::{wrap, PEER_LOG_INTERVAL};
use base::message_proto::Message;
use ddpro::{dd_pro, reject, FormatCaps};
use hbb_common::throttled_log;
use serde_json::{json, Map, Value};

/// 登录应答的名片：只写专业协议版本，详细能力在请求 / 答复里谈。
pub fn add_name_card(platform_additions: &mut Map<String, Value>) {
    platform_additions.insert(
        ddpro::NAME_CARD_KEY.to_owned(),
        json!(ddpro::PROTOCOL_VERSION),
    );
}

/// 处理主控端发来的专业协议消息，返回要回给它的消息。
pub fn on_message(data: &[u8]) -> Option<Message> {
    let msg = match ddpro::decode(data) {
        Ok(msg) => msg,
        Err(err) => {
            throttled_log!(
                PEER_LOG_INTERVAL,
                warn,
                "ddpro: malformed pro protocol message from the controller: {err}"
            );
            return wrap(reject(reject::Reason::MALFORMED, err.to_string()));
        }
    };
    if let Some(mismatch) = ddpro::version_mismatch(msg.version) {
        throttled_log!(
            PEER_LOG_INTERVAL,
            error,
            "ddpro: refusing the controller's request: {mismatch}"
        );
        return wrap(reject(reject::Reason::VERSION_MISMATCH, mismatch));
    }
    let Some(dd_pro::Union::Hello(hello)) = msg.union else {
        throttled_log!(
            PEER_LOG_INTERVAL,
            warn,
            "ddpro: ignoring an unexpected pro protocol message from the controller"
        );
        return None;
    };

    let answer = ddpro::answer_hello(&hello, &encode_caps());
    match &answer.reply {
        dd_pro::Union::Accept(accept) if accept.format.is_some() => throttled_log!(
            PEER_LOG_INTERVAL,
            info,
            "ddpro: pro session accepted, format {}",
            ddpro::describe_format(&accept.format)
        ),
        dd_pro::Union::Accept(_) => throttled_log!(
            PEER_LOG_INTERVAL,
            info,
            "ddpro: pro protocol agreed (v{}); capabilities not probed yet: {}",
            ddpro::PROTOCOL_VERSION,
            answer.unchecked.join(", ")
        ),
        dd_pro::Union::Reject(r) => throttled_log!(
            PEER_LOG_INTERVAL,
            warn,
            "ddpro: pro session refused ({:?}): {}",
            r.reason,
            r.detail
        ),
        _ => {}
    }
    wrap(answer.reply)
}

/// 本机此刻能编的格式。编码器探测在第 3 步接入，在那之前如实报「未探测」。
fn encode_caps() -> FormatCaps {
    FormatCaps::default()
}
