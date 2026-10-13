//! 产品名：投产必须用自己的产品名称。
//!
//! RustDesk 的安装目录、安装后的程序名、Windows 服务名、配置目录、进程间通信管道名都由
//! 「应用名」派生，所以改名就是在任何代码读它之前把它设成我们的名字。
//! 应用名不等于 `RustDesk` 时，RustDesk 会按「定制客户端」行事（不检查官方更新、主页显示
//! Powered by RustDesk 等），这是它给品牌版本设计的正规路径。

use hbb_common::config;

pub const APP_NAME: &str = "dreamdesk-pro";

/// 在程序入口最早处调用，先于 RustDesk 读应用名、加载定制配置。
pub fn apply() {
    *config::APP_NAME.write().unwrap() = APP_NAME.to_owned();
}
