//! 产品名：投产必须用自己的产品名称。
//!
//! RustDesk 的安装目录、安装后的程序名、Windows 服务名、配置目录、进程间通信管道名都由
//! 「应用名」派生，所以改名就是在任何代码读它之前把它设成我们的名字。
//! 应用名不等于 `RustDesk` 时，RustDesk 会按「定制客户端」行事（不检查官方更新、主页显示
//! Powered by RustDesk 等），这是它给品牌版本设计的正规路径。
//!
//! 产品版只在内网用（企业内部禁止连外网）：RustDesk 默认会连官方注册 / 中继 / 账号服务器和写死的公网
//! STUN，这里把服务器预置成内网的（RustDesk 的「默认设置」，官方定制客户端用的同一层：没改过就用它，
//! 设置里「ID/中继服务器」照样能改——用户 2026-10-21 要求放开，原来用「强制设置」锁死、那一栏是灰的），
//! 并强制关掉会去连公网 STUN 的两项。

use base::config::keys;
use hbb_common::config;

pub const APP_NAME: &str = "dreamdesk-pro";

const INTRANET_SERVER: &str = "dreamdesk.of3d.com";
const INTRANET_API: &str = "https://dreamdesk.of3d.com";
/// 内网 RustDesk 服务端（192.168.0.208 上的 hbbs，2026-10-13 部署 1.1.16）的公钥，客户端凭它认服务器。
/// 服务端重装、换了密钥就要改这里并发新版。
const INTRANET_SERVER_KEY: &str = "k0JPM24w59r7hkKuSfjwcGQAxezVb+sTBj5+UZOOlOg=";
/// 被控端应答别人发来的 WebRTC 邀请时不看本机开关；没配 STUN 时 RustDesk 会补上写死的公网 STUN，
/// 配一个内网的就不补了（内网暂时没有 STUN 服务，查询在内网失败，不影响别的连接方式）
const INTRANET_STUN: &str = "stun:dreamdesk.of3d.com:3478";

/// 在程序入口最早处调用，先于 RustDesk 读应用名、加载定制配置。
pub fn apply() {
    *config::APP_NAME.write().unwrap() = APP_NAME.to_owned();
    config::DEFAULT_SETTINGS.write().unwrap().extend(
        [
            (keys::OPTION_CUSTOM_RENDEZVOUS_SERVER, INTRANET_SERVER),
            (keys::OPTION_RELAY_SERVER, INTRANET_SERVER),
            (keys::OPTION_KEY, INTRANET_SERVER_KEY),
            (keys::OPTION_API_SERVER, INTRANET_API),
            (keys::OPTION_ICE_SERVERS, INTRANET_STUN),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned())),
    );
    // 注册服务器不是官方的时候 RustDesk 已把这两项默认关掉，但用户手动打开后仍会去连写死的公网 STUN，所以强制关
    config::OVERWRITE_LOCAL_SETTINGS.write().unwrap().extend(
        [keys::OPTION_ENABLE_IPV6_PUNCH, keys::OPTION_ENABLE_WEBRTC].map(|k| (k.to_owned(), "N".to_owned())),
    );
}

/// 产品版只维护简体中文和英文（用户 2026-10-19 定）：设置页的语言列表只给这两种。
pub fn lang_offered(code: &str) -> bool {
    matches!(code, "en" | "zh-cn")
}

/// 按系统语言自动选、或配置里存着别的语言时：中文（含繁体）用简体中文，其余一律英文。
pub fn product_lang(lang: String) -> String {
    if lang.starts_with("zh") { "zh-cn" } else { "en" }.to_owned()
}
