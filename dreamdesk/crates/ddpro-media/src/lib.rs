//! 专业画面通道：UDP 直连 + RTP（H.265，RFC 7798）+ SRTP（AES-128-GCM）。
//! 与 RustDesk 无关、各平台通用；被控端用 [`channel::Listener`] / [`channel::Sender`]，
//! 主控端用 [`channel::Receiver`]。

pub mod channel;
pub mod hevc;

pub use channel::{Error, Event, Keys, Listener, Receiver, RecvStats, SendStats, Sender};
