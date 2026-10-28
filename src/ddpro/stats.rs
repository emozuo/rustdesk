//! 主控端按连接记下的「连接详情」（界面信号格详情里的 IP、采集方式、硬编，以及连接时长），界面经
//! `session_get_common("dd-net-stats")` 查。
//!
//! 数据来源：连上的时刻和本机这头的地址（收到登录应答时）、被控端报的采集方式和是不是硬编（`VideoInfo`，原版 RustDesk
//! 被控端不发，界面就没有这几行）、被控端 IP（网关给的，主窗口连接前记成对端选项 `dd-ip`）。本机解码是硬是软不在这里：
//! 它只取决于画面编码，界面从 RustDesk 每秒推的质量数据（`codec_format`）自己判断。算好存着，查的时候只读出来拼 JSON。

use crate::{
    client::Interface,
    ui_session_interface::{InvokeUiSession, Session},
};
use lazy_static::lazy_static;
use serde_json::{Map, Value};
use std::{collections::HashMap, net::SocketAddr, sync::Mutex, time::Instant};

#[derive(Default)]
struct Peer {
    /// 这次连接连上的时刻：连接时长从这里算，重连就重新算（照 UU：时长是这一次连接的）。
    /// 通用窗口的标题栏 / 质量监测和专业窗口的悬浮球读的是同一个
    connected_at: Option<Instant>,
    /// 这条连接在本机这一头的地址（直连是连到被控端的网卡地址，走中继是连到中继的网卡地址，都是本机的）
    local_ip: String,
    /// 被控端按屏报的：（采集方式, 是不是硬编）
    video: HashMap<u32, (String, bool)>,
}

lazy_static! {
    /// 按对端设备号（同一对端的多个窗口共用）。断开后不删：下次连这台时整条换掉，量很小
    static ref PEERS: Mutex<HashMap<String, Peer>> = Default::default();
}

/// 远程桌面会话连上（含重连）、收到被控端的登录应答时：清掉上一次连接留下的，记下连上的时刻和本机这头的地址
pub fn on_connected<T: InvokeUiSession>(session: &Session<T>, local: SocketAddr) {
    if !session.is_default() {
        return;
    }
    PEERS.lock().unwrap().insert(
        session.get_id(),
        Peer {
            connected_at: Some(Instant::now()),
            local_ip: local.ip().to_string(),
            ..Default::default()
        },
    );
}

impl Peer {
    /// 这次连接连了多少秒；还没连上为空
    fn connected_secs(&self) -> Option<u64> {
        self.connected_at.map(|t| t.elapsed().as_secs())
    }
}

/// 这次连接连了多少秒；还没连上为 0（专业窗口的悬浮球状态块；通用窗口经 [`query`] 读同一个）
#[cfg(all(feature = "flutter", target_os = "linux"))]
pub fn connected_secs<T: InvokeUiSession>(session: &Session<T>) -> u32 {
    connected_secs_of(&session.get_id()).map_or(0, |s| s.min(u32::MAX as u64) as u32)
}

/// 按对端设备号查这次连接连了多少秒（主窗口首页的会话列表用，见 [`super::sessions`]）；没连上过为空
#[cfg(feature = "flutter")]
pub(super) fn connected_secs_of(id: &str) -> Option<u64> {
    PEERS.lock().unwrap().get(id).and_then(Peer::connected_secs)
}

/// 被控端报的这块屏的采集方式、是不是硬编
pub fn on_video_info<T: InvokeUiSession>(session: &Session<T>, display: u32, capture: &str, hardware: bool) {
    PEERS
        .lock()
        .unwrap()
        .entry(session.get_id())
        .or_default()
        .video
        .insert(display, (capture.to_owned(), hardware));
}

/// 界面的查询：`param` 是窗口现在看的屏号（看全部显示屏时为空，取报过的第一块）。返回 JSON 对象的字符串，拿不到的字段不放
pub fn query<T: InvokeUiSession>(session: &Session<T>, param: &str) -> String {
    let mut out = Map::new();
    let peer_ip = session.get_option("dd-ip".to_owned());
    if !peer_ip.is_empty() {
        out.insert("peer_ip".to_owned(), Value::String(peer_ip));
    }
    let peers = PEERS.lock().unwrap();
    if let Some(peer) = peers.get(&session.get_id()) {
        if let Some(secs) = peer.connected_secs() {
            out.insert("connected_secs".to_owned(), Value::from(secs));
        }
        if !peer.local_ip.is_empty() {
            out.insert("local_ip".to_owned(), Value::String(peer.local_ip.clone()));
        }
        let display = param.parse::<u32>().ok().or_else(|| peer.video.keys().min().copied());
        if let Some((capture, hardware)) = display.and_then(|d| peer.video.get(&d)) {
            if !capture.is_empty() {
                out.insert("capture".to_owned(), Value::String(capture.clone()));
            }
            out.insert(
                "encoder".to_owned(),
                Value::String(if *hardware { "hw" } else { "sw" }.to_owned()),
            );
        }
    }
    Value::Object(out).to_string()
}
