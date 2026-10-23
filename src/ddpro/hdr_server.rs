//! 悬浮球「HDR」（v11）的被控端：按主控端的要求打开 / 关掉这次会话用到的屏的系统 HDR（Windows「使用 HDR」那个开关），
//! 记下每块屏被这条连接改之前的样子，连接关闭时恢复（用户 2026-10-23 定：断开后恢复成连接前的样子）。
//!
//! 两条连接改同一块屏时各记各的，后关的那条恢复成它改之前的样子——少见，记已知限制。

use ddpro::HdrResult;
use hbb_common::log;
use scrap::Display;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

/// 连接号 → 这条连接改过的屏（GDI 名, 改之前开没开 HDR）
fn changed() -> &'static Mutex<HashMap<i32, Vec<(String, bool)>>> {
    static S: OnceLock<Mutex<HashMap<i32, Vec<(String, bool)>>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// RustDesk 屏号对应的 GDI 名（和专业画面采集用的同一份屏列表）
fn name(display: usize) -> Option<String> {
    Display::all().ok()?.into_iter().nth(display).map(|d| d.name())
}

/// 这块屏能不能开 HDR（显示器支持）：「同意」里带给主控端，悬浮球「HDR」据此决定能不能点
pub fn capable(display: usize) -> bool {
    let Some(name) = name(display) else {
        return false;
    };
    let (supported, how) = ddpro::enc::hdr_supported(&name);
    log::info!("ddpro: hdr: display {display} ({name}) can switch HDR: {supported} ({how})");
    supported
}

/// 主控端要把 `displays` 的 HDR 切成 `on`。在阻塞线程里调：系统切显示模式要一两百毫秒到一秒。
/// 不支持 HDR 的屏跳过、不算失败（多屏时整个会话一起切，只有部分屏支持很正常）
pub fn apply(conn_id: i32, on: bool, displays: &[u32]) -> HdrResult {
    let mut problems = Vec::new();
    for &d in displays {
        let Some(name) = name(d as usize) else {
            problems.push(format!("no display {d}"));
            continue;
        };
        if !ddpro::enc::hdr_supported(&name).0 {
            log::info!("ddpro: hdr: display {d} ({name}) does not support HDR, skipped");
            continue;
        }
        match ddpro::enc::set_hdr(&name, on) {
            Ok(was) if was != on => {
                log::info!("ddpro: hdr: turned HDR {} on display {d} ({name}) for connection {conn_id}", if on { "on" } else { "off" });
                let mut changed = changed().lock().unwrap();
                let list = changed.entry(conn_id).or_default();
                // 只记第一次改之前的样子：开了又关、关了又开，断开时都回到连接前
                if !list.iter().any(|(n, _)| *n == name) {
                    list.push((name, was));
                }
            }
            Ok(_) => {}
            Err(e) => {
                log::warn!("ddpro: hdr: {e}");
                problems.push(e);
            }
        }
    }
    HdrResult {
        on,
        ok: problems.is_empty(),
        detail: problems.join("; "),
        ..Default::default()
    }
}

/// 连接关闭：把这条连接改过的屏恢复成它改之前的样子。另起线程做（切显示模式会阻塞，调用方在 RustDesk 的消息循环里）
pub fn on_conn_closed(conn_id: i32) {
    let Some(list) = changed().lock().unwrap().remove(&conn_id) else {
        return;
    };
    let spawned = std::thread::Builder::new().name("ddpro-hdr-restore".into()).spawn(move || {
        for (name, was) in list {
            match ddpro::enc::set_hdr(&name, was) {
                Ok(_) => log::info!("ddpro: hdr: connection {conn_id} closed, HDR on {name} back {}", if was { "on" } else { "off" }),
                Err(e) => log::warn!("ddpro: hdr: cannot restore HDR on {name} after connection {conn_id} closed: {e}"),
            }
        }
    });
    if let Err(e) = spawned {
        log::error!("ddpro: hdr: cannot start the thread that restores HDR: {e}");
    }
}
