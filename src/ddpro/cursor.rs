//! 「使用被控端鼠标」（相对鼠标）期间，把被控端光标的实际位置和「被程序藏起来了」报回给主控端（被控端 Windows）。
//!
//! RustDesk 的光标位置消息只发「不是本连接挪的」位置（`input_service::run_pos` 排除了刚注入过输入的连接）：绝对鼠标时对端
//! 自己知道指针在哪，不用报。相对鼠标时主控端只发位移、位置由被控端系统（含指针加速）决定，对端本机指针又藏着，
//! 不回报就一个指针都看不到。所以开着相对鼠标的连接（专业窗口、通用窗口都算），在这里单独按实际位置回报；
//! `dd_hidden`：三维软件拖动旋转时会把光标藏起来（`CURSOR_SHOWING` 关掉，或光标句柄为空），主控端据此不画，松手再出现。
//! 只对开着相对鼠标的连接生效；其它连接和 RustDesk 原来的行为不变。位置只用来显示，不参与任何判断。
//!
//! 光标位置服务（`mouse_pos`）默认不给连接订阅（没开「显示远程光标」时），没订阅者时服务不跑、`send_to` 也不发给非订阅者，
//! 所以连接开始发相对移动时由 `connection.rs` 的 `dd_follow_cursor` 订阅、改回绝对移动时退订；订阅后 `run_pos` 照原规则
//! 工作（刚注入过输入的连接排除 300 毫秒，所以相对移动期间的位置全由这里回报；停下 300 毫秒后程序自己挪光标，两路可能各发一条
//! 完全相同的位置，无害，不为它加状态）。

use crate::server::{input_service::relative_mouse_conns, service::EmptyExtraFieldService};
use base::message_proto::{message, CursorPosition, Message};
use hbb_common::log;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// 轮询间隔：比 `run_pos` 的 33 毫秒快，相对移动时指针才跟得上；位置没变不发。
/// 再快也没用：主控端会话进程收到每条位置都会推给 Flutter 的事件流，约 60 条 / 秒已经够流畅
const INTERVAL: Duration = Duration::from_millis(16);

static RUNNING: AtomicBool = AtomicBool::new(false);

/// 要回报的连接：开着相对鼠标的（专业窗口、通用窗口的「使用被控端鼠标」都是）
fn active() -> Vec<i32> {
    relative_mouse_conns()
}

/// 被控端光标现在是不是被程序藏起来了（取不到时当没藏）
fn hidden() -> bool {
    matches!(crate::get_cursor(), Ok(None) | Ok(Some(0)))
}

/// `run_pos` 每一拍调用：有连接需要回报了就起回报线程（没有就什么也不做）
pub fn tick(sp: &EmptyExtraFieldService) {
    if RUNNING.load(Ordering::Acquire) || active().is_empty() || RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let sp = sp.clone();
    let spawned = std::thread::Builder::new().name("ddpro-cursor-report".into()).spawn(move || {
        // 每条连接上次发出去的 (x, y, 藏着)
        let mut sent: HashMap<i32, (i32, i32, bool)> = HashMap::new();
        loop {
            let conns = active();
            if conns.is_empty() {
                break;
            }
            if let Some((x, y)) = crate::get_cursor_pos() {
                let now = (x, y, hidden());
                for c in &conns {
                    if sent.get(c) == Some(&now) {
                        continue;
                    }
                    sent.insert(*c, now);
                    let mut msg = Message::new();
                    msg.set_cursor_position(CursorPosition {
                        x,
                        y,
                        dd_hidden: now.2,
                        ..Default::default()
                    });
                    sp.send_to(msg, *c);
                }
            }
            sent.retain(|c, _| conns.contains(c));
            std::thread::sleep(INTERVAL);
        }
        RUNNING.store(false, Ordering::Release);
    });
    if let Err(e) = spawned {
        log::error!("ddpro: cannot start the cursor report thread: {e}");
        RUNNING.store(false, Ordering::Release);
    }
}

/// 普通的光标位置消息（别的东西挪了光标）也带上「藏着」，免得它盖掉回报线程刚发的藏起状态。只在有连接开着相对鼠标时查
pub fn mark_hidden(msg: &mut Message) {
    if active().is_empty() {
        return;
    }
    if let Some(message::Union::CursorPosition(p)) = msg.union.as_mut() {
        p.dd_hidden = hidden();
    }
}
