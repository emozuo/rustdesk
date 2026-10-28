//! 主窗口首页的「本机正开着的会话」（原型 HomeControlling：左栏会话数角标、详情页「被控中」横条和正在连接的设备列表）。
//!
//! 会话就是 RustDesk 的会话表（`flutter::sessions`，一台一种连接一条，同一台多个窗口共用一条；专业模式用的也是这一条），
//! 这里只多记它从什么时候开始：建会话时记下（同一台同一种已经有了就不动），界面经 `main_get_common_sync("dd-sessions")` 查。
//! 远程控制的时长和远程窗口标题栏一致，用连上的时刻（[`super::stats`]，重连就重新算）。

use crate::flutter::sessions;
use hbb_common::rendezvous_proto::ConnType;
use lazy_static::lazy_static;
use serde_json::json;
use std::{collections::HashMap, sync::Mutex, time::Instant};

lazy_static! {
    /// （对端设备号, 连接类型）→ 建会话的时刻。会话关了不在这里删：列的时候顺手清掉会话表里已经没有的
    static ref STARTED: Mutex<HashMap<(String, ConnType), Instant>> = Default::default();
}

/// 建会话时（RustDesk 的 `session_add`，在放进会话表之前）：这台这种连接还没有会话，就是新开的，记下时刻；
/// 已经有了（同一台多开一个窗口）不动
pub fn on_added(id: &str, conn_type: ConnType) {
    if sessions::get_session_by_peer_id(id.to_owned(), conn_type).is_none() {
        STARTED.lock().unwrap().insert((id.to_owned(), conn_type), Instant::now());
    }
}

/// 本机现在开着的会话，JSON 数组：`[{"id": 设备号, "type": "remote|file|port|terminal|camera", "secs": 开了多少秒}]`
pub fn list() -> String {
    let keys = sessions::dd_keys();
    let mut started = STARTED.lock().unwrap();
    started.retain(|k, _| keys.contains(k));
    let items: Vec<_> = keys
        .into_iter()
        .map(|(id, conn_type)| {
            let at = *started.entry((id.clone(), conn_type)).or_insert_with(Instant::now);
            let mut secs = at.elapsed().as_secs();
            // 连上的时刻是上一次连接留下的（还没连上）会比建会话早，取晚的那个
            if conn_type == ConnType::DEFAULT_CONN {
                if let Some(connected) = super::stats::connected_secs_of(&id) {
                    secs = secs.min(connected);
                }
            }
            let kind = match conn_type {
                ConnType::FILE_TRANSFER => "file",
                ConnType::PORT_FORWARD | ConnType::RDP => "port",
                ConnType::TERMINAL => "terminal",
                ConnType::VIEW_CAMERA => "camera",
                _ => "remote",
            };
            json!({ "id": id, "type": kind, "secs": secs })
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}
