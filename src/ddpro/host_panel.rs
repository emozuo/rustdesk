//! 受管版被控端面板的数据（用户 2026-10-28 定：去掉 RustDesk 连接管理小窗，改成主窗口里的「本机被控中」面板）。
//!
//! 连接信息只在连接管理器（`--cm` 进程）里：它还管被控端收文件落盘、文件剪贴板、语音通话，进程一死所有连接会被一起断掉，
//! 所以 cm 照常起，只是窗口永远藏着；主窗口是另一个进程，经本机的 `_cm_panel` 通道向它问列表、让它断开连接。
//! 通道只收「列表 / 断开」两种请求；权限和 `_cm` 同等（Windows 默认 ACL：只有本人 / SYSTEM / 管理员能连，Linux 套接字 0600 按用户）。
//! cm 里没有连接时约 6 秒后进程退出，通道随之消失，主窗口问不通就当没有连接。

use crate::ipc::{self, Data};
use hbb_common::{allow_err, log, tokio};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Mutex};

const POSTFIX: &str = "_cm_panel";
/// 一问一答的连接，主窗口不问了就让它断
const IDLE_MS: u64 = 5_000;
/// 对方自报的电脑名只用来显示，截短免得撑坏界面
const DEVICE_MAX_CHARS: usize = 64;

struct Info {
    account: String,
    device: String,
    platform: String,
    since: i64,
}

lazy_static::lazy_static! {
    static ref INFO: Mutex<HashMap<i32, Info>> = Default::default();
}

/// cm 收到被控端发来的新连接时记下：票里签名的账号名、对方自报的电脑名、起点
pub fn on_login(id: i32, account: String, device: String, platform: String) {
    let device: String = device.chars().filter(|c| !c.is_control()).take(DEVICE_MAX_CHARS).collect();
    let platform: String = platform.chars().filter(|c| !c.is_control()).take(DEVICE_MAX_CHARS).collect();
    INFO.lock().unwrap().insert(
        id,
        Info {
            account,
            device,
            platform,
            since: hbb_common::get_time() / 1000,
        },
    );
}

/// cm 里目前的连接，加上 `dd_account` / `dd_device` / `dd_platform` / `dd_since`（Unix 秒）
fn list_json() -> String {
    let state = crate::ui_cm_interface::get_clients_state();
    let Ok(Value::Array(mut clients)) = serde_json::from_str::<Value>(&state) else {
        return "[]".to_owned();
    };
    let mut info = INFO.lock().unwrap();
    let alive: Vec<i32> = clients.iter().filter_map(|c| c["id"].as_i64().map(|i| i as i32)).collect();
    info.retain(|id, _| alive.contains(id));
    for c in clients.iter_mut() {
        let Some(i) = c["id"].as_i64().and_then(|id| info.get(&(id as i32))) else {
            continue;
        };
        c["dd_account"] = json!(i.account);
        c["dd_device"] = json!(i.device);
        c["dd_platform"] = json!(i.platform);
        c["dd_since"] = json!(i.since);
    }
    Value::Array(clients).to_string()
}

/// 断开某个连接；-1 是全部。和原来小窗上的「断开」同一条路（`ui_cm_interface::close`）
fn close(id: i32) {
    if id >= 0 {
        crate::ui_cm_interface::close(id);
        return;
    }
    let state = crate::ui_cm_interface::get_clients_state();
    if let Ok(Value::Array(clients)) = serde_json::from_str::<Value>(&state) {
        for c in clients.iter().filter(|c| c["disconnected"] != json!(true)) {
            if let Some(id) = c["id"].as_i64() {
                crate::ui_cm_interface::close(id as i32);
            }
        }
    }
}

/// cm 进程里跑：开通道、答主窗口
pub async fn serve() {
    let mut incoming = match ipc::new_listener(POSTFIX).await {
        Ok(i) => i,
        Err(e) => {
            log::error!("dd host panel: ipc listener failed: {e}");
            return;
        }
    };
    while let Some(r) = incoming.next().await {
        match r {
            Ok(stream) => {
                tokio::spawn(answer(ipc::Connection::new(stream)));
            }
            Err(e) => log::error!("dd host panel: accept failed: {e}"),
        }
    }
}

async fn answer(mut conn: ipc::Connection) {
    loop {
        match conn.next_timeout(IDLE_MS).await {
            Ok(Some(Data::DdHostList(None))) => {
                allow_err!(conn.send(&Data::DdHostList(Some(list_json()))).await);
            }
            Ok(Some(Data::DdHostClose(id))) => close(id),
            _ => break,
        }
    }
}

/// 主窗口里调：问 cm 现在谁连着。cm 没起 / 问不通返回空列表
#[tokio::main(flavor = "current_thread")]
pub async fn list() -> String {
    async fn ask() -> hbb_common::ResultType<String> {
        let mut c = ipc::connect(300, POSTFIX).await?;
        c.send(&Data::DdHostList(None)).await?;
        match c.next_timeout(1_000).await? {
            Some(Data::DdHostList(Some(s))) => Ok(s),
            _ => Ok("[]".to_owned()),
        }
    }
    ask().await.unwrap_or_else(|_| "[]".to_owned())
}

/// 主窗口里调：让 cm 断开某个连接（-1 是全部）
#[tokio::main(flavor = "current_thread")]
pub async fn request_close(id: i32) {
    async fn send(id: i32) -> hbb_common::ResultType<()> {
        let mut c = ipc::connect(300, POSTFIX).await?;
        c.send(&Data::DdHostClose(id)).await
    }
    allow_err!(send(id).await);
}

/// cm 里调：把主窗口拉到前面，没开就启动。和托盘「打开」是同一套（Windows 的 runner 只允许一个实例，已有窗口就置前）
pub fn open_main_window() {
    #[cfg(target_os = "linux")]
    {
        // 已开着的窗口经 dbus 置前，没开才新启动（Linux 的窗口不是单实例）
        if crate::dbus::invoke_new_connection(crate::get_uri_prefix()).is_err() {
            allow_err!(crate::run_me::<&str>(vec![]));
        }
    }
    #[cfg(not(target_os = "linux"))]
    allow_err!(crate::run_me::<&str>(vec![]));
}
