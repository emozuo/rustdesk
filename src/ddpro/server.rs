//! 被控端：递名片、答复主控端的请求、开专业画面通道、处理关键帧请求。

use super::wire::{wrap, PEER_LOG_INTERVAL};
use crate::server::{ConnInner, ServerPtrWeak};
use base::message_proto::Message;
use ddpro::{dd_pro, reject, FormatCaps, Hello};
use hbb_common::{log, throttled_log, tokio};
use serde_json::{json, Map, Value};
use std::net::IpAddr;

/// 专业视频服务名的前缀（后面接屏号）
pub(super) const PRO_SERVICE_PREFIX: &str = "ddpro_monitor";

/// 是不是专业视频服务。RustDesk 在连接登录时把它订阅到所有服务上（只按名字跳过它自己的视频服务）；
/// 专业视频服务只能在谈成专业会话时显式订阅，不然只看普通画面的连接也会挂在上面（服务空转、显示变化消息发错人）。
pub fn is_pro_service(name: &str) -> bool {
    name.starts_with(PRO_SERVICE_PREFIX)
}

/// 登录应答的名片：只写专业协议版本，详细能力在请求 / 答复里谈。
pub fn add_name_card(platform_additions: &mut Map<String, Value>) {
    platform_additions.insert(
        ddpro::NAME_CARD_KEY.to_owned(),
        json!(ddpro::PROTOCOL_VERSION),
    );
}

/// 处理专业协议消息时需要的连接信息（由 RustDesk 的连接对象交过来）。
// 不带专业视频的构建（非 Windows，或没开 vram）里，开通道用的字段没人读
#[cfg_attr(not(all(windows, feature = "vram")), allow(dead_code))]
pub struct ConnCtx {
    pub conn_id: i32,
    pub conn: ConnInner,
    pub server: ServerPtrWeak,
    /// RustDesk 连接在被控端这一侧的地址：主控端连得到它，画面通道就开在这个地址上
    pub local_ip: IpAddr,
}

/// 处理主控端发来的专业协议消息，返回要回给它的消息。
/// 探测编码器、开端口要一两百毫秒，放到阻塞线程里做，不卡 RustDesk 的异步运行时。
pub async fn on_message(data: &[u8], ctx: ConnCtx) -> Option<Message> {
    let data = data.to_vec();
    match tokio::task::spawn_blocking(move || handle(&data, &ctx)).await {
        Ok(reply) => reply,
        Err(e) => {
            log::error!("ddpro: pro protocol handler failed: {e}");
            None
        }
    }
}

fn handle(data: &[u8], ctx: &ConnCtx) -> Option<Message> {
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
    match msg.union {
        Some(dd_pro::Union::Hello(hello)) => wrap(answer(&hello, ctx)),
        _ => {
            throttled_log!(
                PEER_LOG_INTERVAL,
                warn,
                "ddpro: ignoring an unexpected pro protocol message from the controller"
            );
            None
        }
    }
}

fn answer(hello: &Hello, ctx: &ConnCtx) -> dd_pro::Union {
    let display = hello.display_index as usize;
    if let Some(why) = busy(display, ctx) {
        throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: pro session refused (DISPLAY_BUSY): {why}");
        return reject(reject::Reason::DISPLAY_BUSY, why);
    }

    let answer = ddpro::answer_hello(hello, &encode_caps(display));
    let mut reply = answer.reply;
    if let dd_pro::Union::Accept(accept) = &mut reply {
        if accept.format.is_some() {
            match open_channel(display, ctx) {
                Ok(media) => accept.media = Some(media).into(),
                Err(e) => {
                    let why = format!("cannot start pro video on display {display}: {e}");
                    throttled_log!(PEER_LOG_INTERVAL, error, "ddpro: pro session refused (UNAVAILABLE): {why}");
                    return reject(reject::Reason::UNAVAILABLE, why);
                }
            }
        }
    }
    match &reply {
        dd_pro::Union::Accept(a) if a.format.is_some() => throttled_log!(
            PEER_LOG_INTERVAL,
            info,
            "ddpro: pro session accepted on display {display}, format {}, channel {}:{}",
            ddpro::describe_format(&a.format),
            a.media.host,
            a.media.port
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
    reply
}

/// 同一块屏专业模式不共享：已有别的专业会话，或有别的连接在用普通模式看，都拒绝。
fn busy(display: usize, ctx: &ConnCtx) -> Option<String> {
    #[cfg(all(windows, feature = "vram"))]
    if super::video::busy_by_other_pro(display, ctx.conn_id, &ctx.server) {
        return Some(format!("display {display} already has a pro session"));
    }
    let viewers = ctx
        .server
        .upgrade()
        .map(|s| s.read().unwrap().display_viewers(display, ctx.conn_id))
        .unwrap_or_default();
    (!viewers.is_empty()).then(|| {
        format!(
            "display {display} is being viewed in general mode by {} other connection(s)",
            viewers.len()
        )
    })
}

/// 本机此刻能编的格式（用这块屏的采集器所在的显卡探测）。做不到时把原因写日志，能力里如实报「没有」。
fn encode_caps(display: usize) -> FormatCaps {
    #[cfg(all(windows, feature = "vram"))]
    let probe = super::video::probe(display);
    #[cfg(not(all(windows, feature = "vram")))]
    let probe = {
        let _ = display;
        Err(ddpro::enc::Error::Unsupported(
            "pro video needs Windows and a build with the vram feature".to_owned(),
        ))
    };
    let (caps, why) = ddpro::encoder_caps(probe);
    if let Some(why) = why {
        throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: pro encoding unavailable here: {why}");
    }
    caps
}

#[cfg(all(windows, feature = "vram"))]
fn open_channel(display: usize, ctx: &ConnCtx) -> hbb_common::ResultType<ddpro::MediaChannel> {
    super::video::start(display, ctx.conn_id, ctx.conn.clone(), ctx.server.clone(), ctx.local_ip)
}

#[cfg(not(all(windows, feature = "vram")))]
fn open_channel(_display: usize, _ctx: &ConnCtx) -> hbb_common::ResultType<ddpro::MediaChannel> {
    hbb_common::bail!("pro video needs Windows and a build with the vram feature")
}
