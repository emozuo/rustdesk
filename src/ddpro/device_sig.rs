//! 受管模式的设备签名（dreamdesk-gateway 仓库 docs/协议-连接票与设备签名.md 第三节）：设备发给服务端的请求带上用设备私钥
//! （RustDesk 的设备密钥对）做的签名，服务端凭它认设备、防冒充。在 RustDesk 发 HTTP 请求的公共函数里按路径挂进去，
//! 不用改各个调用点。

/// 被控端服务进程发的这几条一律签
const SIGNED_PATHS: &[&str] = &["/api/sysinfo", "/api/heartbeat", "/api/audit/conn"];

/// 按请求算签名头（`method` 大写，和服务端看到的一样）；不该签、地址解析不了、本机还没有设备密钥时返回空
pub fn headers(method: &str, url: &str, body: &[u8]) -> Vec<(String, String)> {
    let Ok(u) = url::Url::parse(url) else {
        return Vec::new();
    };
    if !SIGNED_PATHS.contains(&u.path()) {
        return Vec::new();
    }
    // 只取已有的：界面进程里取 get_key_pair 在没有时会新生成一对、存进这个用户的配置
    let Some((sk, pk)) = hbb_common::config::Config::get_existing_key_pair() else {
        return Vec::new();
    };
    match ddpro::ticket::device_signature_headers(
        &sk,
        &pk,
        method,
        u.path(),
        u.query().unwrap_or_default(),
        hbb_common::get_time() / 1000,
        body,
    ) {
        Ok(h) => h,
        Err(e) => {
            // 每次心跳都会走到这里，节流
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(600),
                error,
                "dd-managed: device signature for {}: {e}",
                u.path()
            );
            Vec::new()
        }
    }
}
