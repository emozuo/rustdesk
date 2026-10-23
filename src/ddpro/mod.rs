//! DreamDesk 在主 crate 内的接入层：只做接线，协议逻辑在 `dreamdesk/crates/ddpro`。
//! RustDesk 原有文件里只留调用这里的单行钩子。
//!
//! - `dd-pro`：专业模式（`client` / `server` / `video` / `view` / `wire`）
//! - `dd-brand`：产品改名（`brand`），通用模式同样适用，所以单独一个开关

/// 在程序入口最早处调用（先于 RustDesk 读应用名、加载定制配置）：产品改名，和专业模式要的强制设置
pub fn apply() {
    #[cfg(feature = "dd-brand")]
    brand::apply();
    // 「新连接在标签页中打开」强制关：专业会话期间要隐藏整个通用窗口，同一窗口里的别的会话会被一起藏掉，
    // 所以一个连接一个窗口（P1 第 2 项）。不带产品改名的构建同样要（2026-10-20 审查第 1 条）
    #[cfg(feature = "dd-pro")]
    hbb_common::config::OVERWRITE_LOCAL_SETTINGS.write().unwrap().insert(
        base::config::keys::OPTION_ENABLE_OPEN_NEW_CONNECTIONS_IN_TABS.to_owned(),
        "N".to_owned(),
    );
    // 「将我的所有显示器用于远程会话」默认打开（用户 2026-10-21 定）：双屏瘦客户机连上即每块屏一个专业窗口（P1 第 8 项）。
    // 是默认值，用户在设置里关了按用户的
    #[cfg(feature = "dd-pro")]
    hbb_common::config::DEFAULT_DISPLAY_SETTINGS.write().unwrap().insert(
        base::config::keys::OPTION_USE_ALL_MY_DISPLAYS_FOR_THE_REMOTE_SESSION.to_owned(),
        "Y".to_owned(),
    );
}

// 专业窗口的悬浮球和菜单，和专业窗口同条件
#[cfg(all(feature = "dd-pro", feature = "flutter", target_os = "linux"))]
mod ball;
#[cfg(feature = "dd-brand")]
pub mod brand;
#[cfg(feature = "dd-pro")]
pub mod client;
// 受管模式：被控端服务发给服务端的请求带设备签名（验票、要票在 managed.rs / managed_client.rs，分别挂在连接和客户端模块下）
#[cfg(feature = "dd-managed")]
pub mod device_sig;
// 受管模式：HTTP 客户端只信预置的 DreamDesk 根证书、不接受无效证书
#[cfg(feature = "dd-managed")]
pub mod server_tls;
// 受管模式：被控端服务启动时按物理网卡地址自查克隆，是克隆就换设备号和设备密钥
#[cfg(feature = "dd-managed")]
pub mod clone_check;
// 受管模式：应急连接在本机记账，服务端恢复后补传
#[cfg(feature = "dd-managed")]
pub mod emergency_audit;
#[cfg(feature = "dd-pro")]
pub mod server;
// 专业视频要采集模块直接输出显卡纹理，这在 RustDesk 里绑在 vram 功能上（官方 Windows 版本来就开着）
#[cfg(all(feature = "dd-pro", windows, feature = "vram"))]
mod video;
#[cfg(all(feature = "dd-pro", windows, feature = "vram"))]
mod lossless;
// 主控端专业窗口进程（`--pro-view`）：只有 Linux 版（Wayland），并且用 RustDesk Flutter 版的光标解码
#[cfg(all(feature = "dd-pro", feature = "flutter", target_os = "linux"))]
pub mod view;
#[cfg(all(feature = "dd-pro", feature = "flutter", target_os = "linux"))]
mod view_link;
#[cfg(all(feature = "dd-pro", feature = "flutter", target_os = "linux"))]
mod view_lossless;
#[cfg(feature = "dd-pro")]
mod wire;
// 虚拟麦克风（P2 第 2 项）：只在被控端 Windows（驱动 dreamdesk/driver/mic）
#[cfg(all(feature = "dd-pro", windows))]
pub mod mic;
// USB 透传（P2 第 1 项）：被控端只有 Windows（usbip-win2），主控端只有 Linux（内核 usbip-host，root 服务代做特权操作）
#[cfg(all(feature = "dd-pro", windows))]
mod usb_server;
#[cfg(all(feature = "dd-pro", target_os = "linux"))]
mod usb_client;
#[cfg(all(feature = "dd-pro", target_os = "linux"))]
pub mod usb_helper;
// 悬浮球「HDR」（v11）：被控端切 Windows 的系统 HDR；主控端切 GNOME 的（和专业窗口同条件）
#[cfg(all(feature = "dd-pro", windows))]
mod hdr_server;
#[cfg(all(feature = "dd-pro", feature = "flutter", target_os = "linux"))]
mod hdr_client;
