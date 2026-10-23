//! 被控端的系统光标在桌面合成这一层显示 / 隐藏（专业会话截屏期间藏起来，P1 第 14 节）。
//!
//! Windows 有时改用软件方式把光标直接画进桌面画面（拖动窗口时；有的机器主屏平时也这样），DXGI 截屏就把它一起截进去，
//! 主控端看到本机光标之外还有一个慢半拍的。`MagShowSystemCursor` 是 Windows 放大镜自己用的接口，Parsec、Moonlight 这类串流工具
//! 也用它藏系统光标；它不改任何持久设置，调用的进程退出后自动恢复（崩溃也不会把光标一直藏着）。旧 DreamDesk 也是这么做的。

use windows::Win32::UI::Magnification::{MagInitialize, MagShowSystemCursor};

/// 显示（`show`）或隐藏系统光标。第一次调用时初始化放大镜运行库（之后一直留着，进程退出时系统收回）。
/// 放大镜接口有线程归属，调用方要始终在同一个线程里调
pub fn show_system_cursor(show: bool) -> Result<(), String> {
    thread_local! {
        static READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    unsafe {
        if !READY.get() {
            if !MagInitialize().as_bool() {
                return Err(format!("MagInitialize failed: {}", std::io::Error::last_os_error()));
            }
            READY.set(true);
        }
        if MagShowSystemCursor(show).as_bool() {
            Ok(())
        } else {
            Err(format!("MagShowSystemCursor({show}) failed: {}", std::io::Error::last_os_error()))
        }
    }
}
