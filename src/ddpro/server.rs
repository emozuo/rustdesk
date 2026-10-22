//! 被控端：递名片、答复主控端的请求、开专业画面通道（关键帧请求 v4 起走画面通道，不经这里）。

use crate::{
    ddpro::wire::{wrap, PEER_LOG_INTERVAL},
    server::{service::Subscriber, ConnInner, ServerPtrWeak},
};
use base::message_proto::Message;
use ddpro::{dd_pro, reject, FormatCaps, Hello};
use hbb_common::{log, throttled_log, tokio};
use serde_json::{json, Map, Value};
use std::net::IpAddr;

/// 专业视频服务名的前缀（后面接屏号）
const PRO_SERVICE_PREFIX: &str = "ddpro_monitor";

#[cfg(all(windows, feature = "vram"))]
pub(super) fn pro_service_name(display: usize) -> String {
    format!("{PRO_SERVICE_PREFIX}{display}")
}

/// 是不是专业视频服务。RustDesk 在连接登录时把它订阅到所有服务上（只按名字跳过它自己的视频服务）；
/// 专业视频服务只能在谈成专业会话时显式订阅，不然只看通用画面的连接也会挂在上面（服务空转、显示变化消息发错人）。
pub fn is_pro_service(name: &str) -> bool {
    name.starts_with(PRO_SERVICE_PREFIX)
}

/// RustDesk 要给连接订某块屏的普通画面（切屏、多屏开窗都经服务器的订阅入口），而这块屏正由这条连接自己的专业会话占着：
/// 不放行，免得同一块屏编两路（P1 第 8 项）。只看专业服务还订没订着：专业会话一结束（专业服务退订了）就不再拦，退回通用画面照常订。
#[cfg(all(windows, feature = "vram"))]
pub fn holds_display(server: &crate::server::Server, name: &str, conn_id: i32) -> bool {
    use crate::server::video_service::VideoSource;
    let Some(display) = name
        .strip_prefix(VideoSource::Monitor.service_name_prefix())
        .and_then(|d| d.parse::<usize>().ok())
    else {
        return false;
    };
    let held = server.is_subscribed(&pro_service_name(display), conn_id);
    if held {
        throttled_log!(
            PEER_LOG_INTERVAL,
            info,
            "ddpro: keeping the general video of display {display} off: this connection's pro session has it"
        );
    }
    held
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
    pub conn: ConnInner,
    pub server: ServerPtrWeak,
    /// RustDesk 连接在被控端这一侧的地址：主控端连得到它，画面通道就开在这个地址上
    pub local_ip: IpAddr,
}

/// 处理主控端发来的专业协议消息，返回要回给它的消息。
/// 探测编码器、开端口要一两百毫秒，放到阻塞线程里做，不卡 RustDesk 的异步运行时；但这个连接自己的消息循环
/// 在等答复，这期间本连接的通用画面和输入会停一两百毫秒（只发生在开会话、换屏时）。
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
        Some(dd_pro::Union::Hello(hello)) => {
            let mut reply = answer(&hello, ctx);
            match &mut reply {
                dd_pro::Union::Accept(a) => a.display_index = hello.display_index,
                dd_pro::Union::Reject(r) => r.display_index = hello.display_index,
                _ => {}
            }
            wrap(reply)
        }
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
    // 谈判前的早拒绝（免得白探测显卡）；开通道时在服务器锁内还会再查一遍，那次才是准的
    let why = ctx.server.upgrade().and_then(|s| busy(&s.read().unwrap(), display, ctx.conn.id()));
    if let Some(why) = why {
        throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: pro session refused (DISPLAY_BUSY): {why}");
        return reject(reject::Reason::DISPLAY_BUSY, why);
    }

    let answer = ddpro::answer_hello(hello, || encode_caps(display));
    let mut reply = answer.reply;
    if let dd_pro::Union::Accept(accept) = &mut reply {
        if accept.format.is_some() {
            match open_channel(display, ctx) {
                Ok(media) => accept.media = Some(media).into(),
                #[cfg(all(windows, feature = "vram"))]
                Err(e) if e.is::<super::video::Busy>() => {
                    throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: pro session refused (DISPLAY_BUSY): {e}");
                    return reject(reject::Reason::DISPLAY_BUSY, e.to_string());
                }
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

/// 同一块屏专业模式不共享：已有别的专业会话（还订阅着专业服务才算；断掉的会话留下的登记不算），
/// 或有别的连接在用通用模式看，都拒绝，返回原因。
pub(super) fn busy(s: &crate::server::Server, display: usize, conn_id: i32) -> Option<String> {
    #[cfg(all(windows, feature = "vram"))]
    if super::video::other_session(display, conn_id).is_some_and(|id| s.is_subscribed(&pro_service_name(display), id)) {
        return Some(format!("display {display} already has a pro session"));
    }
    let viewers = s.display_viewers(display, conn_id).len();
    (viewers > 0).then(|| format!("display {display} is being viewed in general mode by {viewers} other connection(s)"))
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
    super::video::start(display, ctx.conn.id(), ctx.conn.clone(), ctx.server.clone(), ctx.local_ip)
}

#[cfg(not(all(windows, feature = "vram")))]
fn open_channel(_display: usize, _ctx: &ConnCtx) -> hbb_common::ResultType<ddpro::MediaChannel> {
    hbb_common::bail!("pro video needs Windows and a build with the vram feature")
}
