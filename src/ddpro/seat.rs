//! 座位（kiosk 瘦客户机，用户 10-26 定，dreamdesk/docs/K-kiosk方案.md「座位」）：装了 kiosk 安装包的机器
//! - 不当被控端：不向 ID 服务器登记、不开直连端口和局域网发现，别人往它的设备号上连只看到不在线，应急票也没处用。
//!   和网关的心跳（座位在线、换地址告警、设备登记）照旧——它直接发给网关，不经 ID 服务器；
//! - 上报设备信息时带「我是座位」，服务端据此给待确认的设备预选类型（确认仍由管理员做）。

/// kiosk 安装包带的记号文件：本机是座位。跟着包走，卸掉 kiosk 包就恢复成普通设备；`dreamdesk-kiosk off` 切回桌面不影响
const SEAT_MARKER: &str = "/usr/share/dreamdesk-kiosk/seat";

pub fn is_seat() -> bool {
    std::path::Path::new(SEAT_MARKER).exists()
}
