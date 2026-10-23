//! 受管模式的设备签名（dreamdesk-gateway 仓库 docs/协议-连接票与设备签名.md 第三节）：被控端服务发给服务端的机器信息、
//! 心跳、连接审计带上用设备私钥（RustDesk 的设备密钥对）做的签名，服务端凭它认设备、防冒充。
//! 在 RustDesk 发 HTTP 请求的公共函数里按路径挂进去，不用改各个调用点。

/// 要签名的接口路径。只有被控端服务进程发这几条，它持有设备私钥；别的进程不签（取密钥对在没有时会新生成一对，不能在别的进程里取）
const SIGNED_PATHS: &[&str] = &["/api/sysinfo", "/api/heartbeat", "/api/audit/conn"];

/// 按请求算签名头；路径不在名单里、或地址解析不了时返回空
pub fn headers(method: &str, url: &str, body: &[u8]) -> Vec<(String, String)> {
    let Ok(u) = url::Url::parse(url) else {
        return Vec::new();
    };
    if !SIGNED_PATHS.contains(&u.path()) {
        return Vec::new();
    }
    let (sk, pk) = hbb_common::config::Config::get_key_pair();
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
            hbb_common::log::error!("dd-managed: device signature for {}: {e}", u.path());
            Vec::new()
        }
    }
}
