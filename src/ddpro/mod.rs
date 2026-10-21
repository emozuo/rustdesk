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
}

// 专业窗口的悬浮球和菜单，和专业窗口同条件
#[cfg(all(feature = "dd-pro", feature = "flutter", target_os = "linux"))]
mod ball;
#[cfg(feature = "dd-brand")]
pub mod brand;
#[cfg(feature = "dd-pro")]
pub mod client;
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
