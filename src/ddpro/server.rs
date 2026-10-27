//! 被控端：递名片、答复主控端的请求、开专业画面通道（关键帧请求 v4 起走画面通道，不经这里）。

use crate::{
    ddpro::wire::{wrap, PEER_LOG_INTERVAL},
    server::{
        service::{GenericService, Subscriber},
        ConnInner, ServerPtrWeak,
    },
};
use base::message_proto::Message;
use ddpro::{dd_pro, reject, FormatCaps, HdrResult, Hello, SetHdr};
use hbb_common::{log, throttled_log, tokio};
use serde_json::{json, Map, Value};
use std::{collections::HashSet, net::IpAddr, sync::Arc};

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

/// 发给主控端的屏幕信息里这块屏的显示器型号名（`DisplayInfo.dd_label`，主控端屏幕标签写「显示屏 1（VX2723）」）。
/// RustDesk 每 300 毫秒（有连接时）重建一次屏幕列表，这里跟着查一次系统显示配置（实测每块屏几十微秒，
/// `cargo run -p ddpro-enc --example display_names`）；查不到给空串
/// 偶尔查不到（换显示模式那一下系统显示配置会短暂查失败）时沿用这块屏上次查到的：型号一变 RustDesk 就当屏幕变了，
/// 给所有主控端重发整份屏幕列表，标签也会闪一下。代价是同一个接口换上一台报不出型号的显示器时，标签还写着旧型号
#[cfg(windows)]
pub fn display_label(name: &str) -> String {
    lazy_static::lazy_static! {
        static ref LAST: std::sync::Mutex<std::collections::HashMap<String, String>> = Default::default();
    }
    let mut last = LAST.lock().unwrap();
    match ddpro::enc::friendly_name(name) {
        Some(label) => {
            if last.get(name) != Some(&label) {
                last.insert(name.to_owned(), label.clone());
            }
            label
        }
        None => last.get(name).cloned().unwrap_or_default(),
    }
}

/// 登录应答的名片：只写专业协议版本，详细能力在请求 / 答复里谈。
pub fn add_name_card(platform_additions: &mut Map<String, Value>) {
    platform_additions.insert(
        ddpro::NAME_CARD_KEY.to_owned(),
        json!(ddpro::PROTOCOL_VERSION),
    );
}

/// 通用画面服务（`video_service::run`，每次重启一份新的）把这块屏的采集方式、是不是硬编告诉订阅它的连接
/// （`VideoInfo`，只给主控端界面的详情显示）。
/// 要在画面服务发出一帧之后报（[`Self::on_frame`]）：新订阅者在第一帧发出前还在「新订阅」里，`send_shared` 只送老订阅者；
/// 发出第一帧前 RustDesk 自己的 snapshot 才把它们转正。这里不碰 snapshot——它会提前把新订阅者转正，画面服务就不会为新订阅者重启，
/// 新订阅者拿不到关键帧
pub struct VideoReport {
    /// 摄像头不报
    monitor: bool,
    /// Windows 的采集现在是不是 GDI（DXGI 出错或被选项关掉后退回的）
    gdi: bool,
    /// 已经收到的连接
    informed: HashSet<i32>,
}

impl VideoReport {
    pub fn new(monitor: bool) -> Self {
        Self {
            monitor,
            gdi: false,
            informed: HashSet::new(),
        }
    }

    /// 画面服务建好时采集已经是 GDI 的（选项关了 DXGI、DXGI 恢复用尽）：只记状态，等第一帧发出时一起报
    #[cfg(windows)]
    pub fn set_gdi(&mut self, gdi: bool) {
        self.gdi = gdi;
    }

    /// 每发出一帧调用：这一帧发给的连接里有还没收到的，就报一次。第一个连接、画面服务重启后的连接、中途加入的连接都经这里送到——
    /// 此刻新订阅者已被 `handle_one_frame` 开头的 snapshot 转正，`send_shared` 送得到
    pub fn on_frame(&mut self, sp: &GenericService, display: usize, hardware: bool, conns: &HashSet<i32>) {
        if conns.iter().all(|id| self.informed.contains(id)) {
            return;
        }
        self.informed.extend(conns.iter().copied());
        self.send(sp, display, hardware);
    }

    /// DXGI 中途退回 GDI：对所有订阅者再报一次
    #[cfg(windows)]
    pub fn fell_back_to_gdi(&mut self, sp: &GenericService, display: usize, hardware: bool) {
        self.gdi = true;
        self.send(sp, display, hardware);
    }

    fn send(&self, sp: &GenericService, display: usize, hardware: bool) {
        if !self.monitor {
            return;
        }
        if let Some(msg) = wrap(ddpro::video_info(display as u32, capture_name(self.gdi), hardware)) {
            sp.send_shared(Arc::new(msg));
        }
    }
}

/// 通用画面的采集方式。Linux 的 Wayland 只认 PipeWire 一条路（带 drm 功能的构建在登录界面可能走 DRM，这里不区分）
fn capture_name(_gdi: bool) -> &'static str {
    #[cfg(windows)]
    return if _gdi { "GDI" } else { "DXGI" };
    #[cfg(target_os = "linux")]
    return if crate::platform::linux::is_x11() { "X11" } else { "PipeWire" };
    #[cfg(not(any(windows, target_os = "linux")))]
    ""
}

/// 处理专业协议消息时需要的连接信息（由 RustDesk 的连接对象交过来）。
// 不带专业视频的构建（非 Windows，或没开 vram）里，开通道用的字段没人读
#[cfg_attr(not(all(windows, feature = "vram")), allow(dead_code))]
pub struct ConnCtx {
    pub conn: ConnInner,
    pub server: ServerPtrWeak,
    /// RustDesk 连接在被控端这一侧的地址：主控端连得到它，画面通道就开在这个地址上
    pub local_ip: IpAddr,
    /// 这条连接能输入（被控端给了键鼠控制权、主控端也没开「仅查看」，RustDesk 的 `peer_keyboard_enabled`；USB 透传要它）
    pub keyboard: bool,
}

/// 处理主控端发来的专业协议消息，返回要回给它的消息。
/// 探测编码器、开端口要一两百毫秒，放到阻塞线程里做，不卡 RustDesk 的异步运行时；但这个连接自己的消息循环
/// 在等答复，这期间本连接的通用画面和输入会停一两百毫秒（只发生在开会话、换屏时）。
pub async fn on_message(data: &[u8], ctx: ConnCtx) -> Option<Message> {
    // USB 透传的消息很频繁（数位板每秒上百个）、处理都很快：直接处理，不进阻塞线程、不让消息循环等
    if let Ok(msg) = ddpro::decode(data) {
        if msg.version == ddpro::PROTOCOL_VERSION {
            if let Some(u) = msg.union.filter(is_usb) {
                usb(u, &ctx);
                return None;
            }
        }
    }
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
        Some(dd_pro::Union::SetHdr(s)) => wrap(dd_pro::Union::HdrResult(set_hdr(&s, ctx))),
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

/// 悬浮球「HDR」（v11）：切这次会话用到的屏的系统 HDR（只有 Windows 被控端能切）。要这条连接能输入
/// （和 USB 透传一样）：只看的连接不能改被控机的系统设置
fn set_hdr(s: &SetHdr, _ctx: &ConnCtx) -> HdrResult {
    log::info!("ddpro: hdr: the controller asks to turn HDR {} on display(s) {:?}", if s.on { "on" } else { "off" }, s.displays);
    if !_ctx.keyboard {
        log::warn!("ddpro: hdr: refused, this connection has no keyboard / mouse control");
        return HdrResult {
            on: s.on,
            ok: false,
            detail: "this connection has no keyboard / mouse control".to_owned(),
            ..Default::default()
        };
    }
    #[cfg(windows)]
    return super::hdr_server::apply(_ctx.conn.id(), s.on, &s.displays);
    #[cfg(not(windows))]
    HdrResult {
        on: s.on,
        ok: false,
        detail: "switching HDR needs a Windows controlled side".to_owned(),
        ..Default::default()
    }
}

fn is_usb(u: &dd_pro::Union) -> bool {
    matches!(
        u,
        dd_pro::Union::UsbOffer(_) | dd_pro::Union::UsbData(_) | dd_pro::Union::UsbClose(_)
    )
}

/// USB 透传（P2 第 1 项）：被控端驱动 usbip-win2 只有 Windows 版
fn usb(u: dd_pro::Union, ctx: &ConnCtx) {
    #[cfg(windows)]
    super::usb_server::on_message(u, &ctx.conn, ctx.keyboard);
    #[cfg(not(windows))]
    if let dd_pro::Union::UsbOffer(offer) = u {
        use ddpro::usb_status::{Reason, State};
        for d in offer.devices {
            let s = dd_pro::Union::UsbStatus(ddpro::UsbStatus {
                device_id: d.device_id,
                state: State::REJECTED.into(),
                reason: Reason::DRIVER_MISSING.into(),
                detail: "USB passthrough needs a Windows controlled side".to_owned(),
                ..Default::default()
            });
            if let Some(m) = wrap(s) {
                ctx.conn.clone().send(std::sync::Arc::new(m));
            }
        }
    }
}

/// 这条连接能不能输入变了：被控端在连接管理里切了键鼠控制权，或主控端开 / 关了「仅查看」（`connection.rs` 的钩子）
pub fn on_keyboard_permission(_conn_id: i32, _enabled: bool) {
    #[cfg(windows)]
    super::usb_server::on_keyboard_permission(_conn_id, _enabled);
}

/// `--ddpro-usbip install|uninstall`：产品安装 / 自动升级 / 卸载脚本里调（`platform/windows.rs` 的钩子），以管理员身份运行。
/// 随包的 usbip-win2 安装程序在程序目录的 `usbip` 子目录里（打包脚本放进去）。失败只写日志：没有 USB 透传，产品其余照常
#[cfg(windows)]
pub fn usbip_command(cmd: &str) {
    let result = match cmd {
        "install" => std::env::current_exe()
            .map_err(|e| e.to_string())
            .and_then(|exe| ddpro::usb::vhci::install(&exe.with_file_name("usbip"))),
        "uninstall" => ddpro::usb::vhci::uninstall(),
        _ => Err(format!("unknown --ddpro-usbip command {cmd:?}")),
    };
    match result {
        Ok(done) => log::info!("ddpro: usb: {done}"),
        Err(e) => log::error!("ddpro: usb: --ddpro-usbip {cmd} failed: {e}; no USB passthrough on this machine"),
    }
}

/// 产品安装、自动升级脚本里装 / 升级 USB 透传驱动的那一行（`platform/windows.rs` 的钩子）
#[cfg(windows)]
pub fn usbip_install_cmd(exe: &str) -> String {
    format!("\"{exe}\" --ddpro-usbip install")
}

/// 产品卸载脚本里卸掉 USB 透传驱动的那一行（接在卸打印驱动那一行后面）
#[cfg(windows)]
pub fn usbip_uninstall_cmd(exe: &str) -> String {
    format!("\n    \"{exe}\" --ddpro-usbip uninstall")
}

/// 连接关闭（`connection.rs` 的钩子）
pub fn on_conn_closed(_conn_id: i32) {
    #[cfg(feature = "dd-managed")]
    super::ticket_conns::remove(_conn_id);
    #[cfg(windows)]
    super::usb_server::on_conn_closed(_conn_id);
    #[cfg(windows)]
    super::mic::on_conn_closed(_conn_id);
    #[cfg(windows)]
    super::hdr_server::on_conn_closed(_conn_id);
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
            // 谈成 10bit（只在屏开着 HDR 时谈得到）：现查 HDR，告诉主控端普通内容的白多亮。这期间 HDR 刚好被关了，
            // 就是 10bit 普通画面（两头都按 BT.709 处理）
            if accept.format.bit_depth == 10 {
                if let Some(white) = hdr_white(display) {
                    accept.format.mut_or_insert_default().hdr = true;
                    accept.format.mut_or_insert_default().sdr_white_nits = white;
                }
            }
            #[cfg(windows)]
            {
                accept.hdr_capable = super::hdr_server::capable(display);
            }
            match open_channel(display, ctx, accept.format.bit_depth, accept.format.hdr) {
                Ok(media) => {
                    accept.media = Some(media).into();
                    // 专业画面用 DXGI 桌面复制采集纹理，没有 GDI 退路
                    #[cfg(all(windows, feature = "vram"))]
                    {
                        accept.capture = "DXGI".to_owned();
                    }
                    #[cfg(windows)]
                    super::mic::on_pro_session(ctx.conn.id());
                }
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

/// 本机此刻能编的格式（用这块屏的采集器所在的显卡探测）。做不到时把原因写日志、也交给谈判带给对端，能力里如实报「没有」。
fn encode_caps(display: usize) -> (FormatCaps, Option<String>) {
    #[cfg(all(windows, feature = "vram"))]
    let probe = super::video::probe(display);
    #[cfg(not(all(windows, feature = "vram")))]
    let probe = {
        let _ = display;
        Err(ddpro::enc::Error::Unsupported(
            "pro video needs Windows and a build with the vram feature".to_owned(),
        ))
    };
    let (caps, why) = ddpro::encoder_caps(probe, hdr_white(display).is_some());
    if let Some(why) = &why {
        throttled_log!(PEER_LOG_INTERVAL, warn, "ddpro: pro encoding unavailable here: {why}");
    }
    (caps, why)
}

/// 这块屏开着 HDR 时普通内容的白有多少尼特（P1 第 4 项）
fn hdr_white(display: usize) -> Option<u32> {
    #[cfg(all(windows, feature = "vram"))]
    return super::video::hdr_white(display);
    #[cfg(not(all(windows, feature = "vram")))]
    {
        let _ = display;
        None
    }
}

#[cfg(all(windows, feature = "vram"))]
fn open_channel(display: usize, ctx: &ConnCtx, bit_depth: u32, hdr: bool) -> hbb_common::ResultType<ddpro::MediaChannel> {
    super::video::start(display, ctx.conn.id(), ctx.conn.clone(), ctx.server.clone(), ctx.local_ip, bit_depth, hdr)
}

#[cfg(not(all(windows, feature = "vram")))]
fn open_channel(_display: usize, _ctx: &ConnCtx, _bit_depth: u32, _hdr: bool) -> hbb_common::ResultType<ddpro::MediaChannel> {
    hbb_common::bail!("pro video needs Windows and a build with the vram feature")
}
