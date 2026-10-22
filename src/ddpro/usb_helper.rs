//! 主控端 Linux：root `--service` 里的 USB 透传特权端点（P2 第 1 项）。会话进程是普通用户，绑定 / 解绑驱动、把套接字交给
//! 内核 usbip-host 由它代做；逻辑在 `ddpro_usb::helper`，这里只接 RustDesk：套接字放在 `_service` 的受保护目录里（和 `_drm`
//! 一样），授权用 RustDesk uinput 服务那一套（对端是当前活动会话的用户或 root，并且是同一个程序）。

use hbb_common::{config::Config, log, throttled_log};
use std::{
    os::fd::RawFd,
    path::{Path, PathBuf},
    time::Duration,
};

const POSTFIX: &str = "_ddusb";
const REJECT_LOG_INTERVAL: Duration = Duration::from_secs(5);
/// 端点的日志：会话进程（同一用户的同一程序）连续发坏请求时不刷屏
const HELPER_LOG_INTERVAL: Duration = Duration::from_secs(2);
/// 绑着的设备的记账（root 服务重启时据此收回）：root 自己建的 0700 目录，开机清空，和内核里的登记同生命周期
const STATE_DIR: &str = "/run/ddpro-usb";

fn dir() -> PathBuf {
    let service = PathBuf::from(Config::ipc_path(crate::POSTFIX_SERVICE));
    service.parent().map_or_else(|| PathBuf::from("/tmp"), |p| p.to_owned())
}

/// 端点的套接字路径（会话进程也用它去连）
pub(super) fn socket_path() -> PathBuf {
    dir().join("ipc_ddusb")
}

fn authorize(fd: RawFd) -> bool {
    let peer_uid = crate::ipc::peer_uid_from_fd(fd);
    let active_uid = crate::platform::linux::get_active_userid_fresh().trim().parse::<u32>().ok();
    if !peer_uid.is_some_and(|uid| crate::ipc::is_allowed_service_peer_uid(uid, active_uid)) {
        throttled_log!(
            REJECT_LOG_INTERVAL,
            warn,
            "ddpro: usb helper: rejected a connection from uid {peer_uid:?} (active uid {active_uid:?})"
        );
        return false;
    }
    if let Err(e) = crate::ipc::ensure_peer_executable_matches_current_by_fd(fd, POSTFIX) {
        throttled_log!(REJECT_LOG_INTERVAL, warn, "ddpro: usb helper: rejected a connection: {e}");
        return false;
    }
    true
}

/// 在 root 服务里起端点（`platform/linux.rs` 的钩子）。端点起不来只影响 USB 透传，写日志后放弃
pub fn start() {
    let spawned = std::thread::Builder::new().name("ddpro-usb-helper".into()).spawn(|| {
        // 目录由 `_service` 通道在它自己的线程里建好并收归 root，和本线程同时起：等它好了再往里放东西
        let path = socket_path();
        for _ in 0..60 {
            if ddpro::usb::helper::parent_ready(&path).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let log = |m: &str| throttled_log!(HELPER_LOG_INTERVAL, info, "ddpro: {m}");
        if let Err(e) = ddpro::usb::helper::serve(&path, Path::new(STATE_DIR), authorize, log) {
            log::error!("ddpro: usb helper failed to start: {e}; USB passthrough is off");
        }
    });
    if let Err(e) = spawned {
        log::error!("ddpro: cannot spawn the usb helper thread: {e}; USB passthrough is off");
    }
}
