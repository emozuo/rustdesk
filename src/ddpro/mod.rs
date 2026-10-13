//! DreamDesk 在主 crate 内的接入层：只做接线，协议逻辑在 `dreamdesk/crates/ddpro`。
//! RustDesk 原有文件里只留调用这里的单行钩子。
//!
//! - `dd-pro`：专业模式（`client` / `server` / `wire`）
//! - `dd-brand`：产品改名（`brand`），通用模式同样适用，所以单独一个开关

#[cfg(feature = "dd-brand")]
pub mod brand;
#[cfg(feature = "dd-pro")]
pub mod client;
#[cfg(feature = "dd-pro")]
pub mod server;
#[cfg(feature = "dd-pro")]
mod wire;
