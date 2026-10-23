//! 受管模式：凭票认证、还连着的连接 → 票编号（dreamdesk-gateway 协议第五节）。认证成功发的连接审计带上它；心跳每一拍也带上
//! （`dd_tickets`）：审计是一次性的事件，服务端停的时间长过它的重试窗口就丢了，关联靠每拍刷新的状态才不会断，收回授权才断得掉。

use serde_json::{json, Map, Value};
use std::{collections::HashMap, sync::Mutex};

lazy_static::lazy_static! {
    static ref TICKETS: Mutex<HashMap<i32, String>> = Default::default();
}

/// 验票通过、开始走认证成功的后续之前记下（审计在后续里发）
pub fn insert(conn_id: i32, jti: String) {
    TICKETS.lock().unwrap().insert(conn_id, jti);
}

/// 认证没走成，或者连接关了
pub fn remove(conn_id: i32) {
    TICKETS.lock().unwrap().remove(&conn_id);
}

pub fn get(conn_id: i32) -> Option<String> {
    TICKETS.lock().unwrap().get(&conn_id).cloned()
}

/// 心跳请求体加 `dd_tickets`：只列 `conns`（还连着的）里凭票认证的
pub fn add_to_heartbeat(v: &mut Value, conns: &[i32]) {
    let tickets = TICKETS.lock().unwrap();
    let m: Map<String, Value> = conns
        .iter()
        .filter_map(|c| tickets.get(c).map(|jti| (c.to_string(), json!(jti))))
        .collect();
    if !m.is_empty() {
        v["dd_tickets"] = Value::Object(m);
    }
}
