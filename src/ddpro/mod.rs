//! DreamDesk 在主 crate 内的接入层：只做接线，协议逻辑在 `dreamdesk/crates/ddpro`。
//! RustDesk 原有文件里只留调用这里的单行钩子。
//!
//! - `dd-pro`：专业模式（`client` / `server` / `video` / `view` / `wire`）
//! - `dd-brand`：产品改名（`brand`），通用模式同样适用，所以单独一个开关

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
