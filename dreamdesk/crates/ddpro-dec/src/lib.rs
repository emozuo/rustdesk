//! 主控端专业硬解：VAAPI（Linux）。
//!
//! 链路：H.265 4:4:4 码流 → FFmpeg + VAAPI 硬解（显存里的 YUV 4:4:4）→ 显卡视频处理单元按 BT.709 全范围
//! 转成 RGB（顺带缩放）→ 导出成 dmabuf 交给显示。全程不经 CPU 拷贝；读回只给验证用。
//! 颜色转换放在这里而不是交给合成器：把 YUV 交给合成器时，「按 BT.709 全范围还原」目前没有协议能告诉它
//! （见 P0 实施记录第 5 步）。

#[cfg(target_os = "linux")]
mod ffi;
#[cfg(target_os = "linux")]
mod vaapi;

#[cfg(target_os = "linux")]
pub use vaapi::*;
