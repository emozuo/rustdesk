//! 主控端专业显示：独立原生窗口（Wayland），显存里的 RGB 画面经 dmabuf 交给合成器，零拷贝；
//! 每帧用 presentation-time 拿到实际开始显示的时刻（分段延迟的「上屏」一段）。
//!
//! 只支持 Wayland（P0 决定，见 P0 实施记录第 5 步）；其它平台这个 crate 是空的。

#[cfg(target_os = "linux")]
mod wayland;

#[cfg(target_os = "linux")]
pub use wayland::*;
