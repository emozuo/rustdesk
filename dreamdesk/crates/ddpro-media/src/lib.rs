//! 专业画面通道：UDP 直连 + RTP（H.265，RFC 7798）+ SRTP（AES-128-GCM）+ 端到端分段延迟。
//! 与 RustDesk 无关、各平台通用；被控端用 [`channel::Listener`] / [`channel::Sender`]，
//! 主控端用 [`channel::Receiver`]，分段延迟记账用 [`timing::Ledger`]。

pub mod channel;
pub mod hevc;
pub mod timing;

pub use channel::{Error, Event, Keys, Listener, Receiver, RecvStats, SendStats, Sender, Stamp};
pub use timing::{ClockEstimate, FrameTimes, Ledger, LedgerSummary, Stage};
