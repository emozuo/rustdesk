//! 专业协议消息与 RustDesk Message 之间的装拆，以及收消息时的日志节流。

use base::message_proto::Message;
use hbb_common::log;
use std::time::Duration;

/// 收到对端专业协议消息时的日志节流间隔：正常一个会话只谈一次，但发多少由对端决定，
/// 不节流就能被刷爆日志。10 秒既压得住刷屏，又不至于把挨得近的两次正常会话合并掉。
pub const PEER_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// 把专业协议消息装进 RustDesk 的 Message。编码失败只记日志，调用方拿到 None 就不发。
pub fn wrap(union: ddpro::dd_pro::Union) -> Option<Message> {
    match ddpro::encode(union) {
        Ok(bytes) => {
            let mut msg = Message::new();
            msg.set_dd_pro(bytes.into());
            Some(msg)
        }
        Err(err) => {
            log::error!("ddpro: failed to encode a pro protocol message: {err}");
            None
        }
    }
}
