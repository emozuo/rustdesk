//! 受管模式的服务器证书校验（dreamdesk-gateway 仓库 docs/S1-方案.md 第五节第 9 条）：RustDesk 的网络请求出错时会改成
//! 「接受无效证书」重试并记住，登录口令、令牌、票、心跳下发的设置都经过它。受管构建里所有 HTTP 客户端只信任预置的
//! DreamDesk 根证书（产品服务器的证书由它签发），不信任系统和内置的根证书，也不接受无效证书。
//! 产品是内网专用，受管构建里没有别的 HTTPS 目的地，所以 RustDesk 建 HTTP 客户端的两个函数在受管构建里整个交给这里。
//!
//! 和 RustDesk 原来的建法（`hbbs_http::http_client` 的宏）一样用配置的代理；不同的是任何一步失败都不退回信系统根证书的
//! 默认客户端：代理设不上就直连，连固定证书的客户端都建不出来就退出进程。

use hbb_common::{
    config::Config,
    log,
    proxy::{Proxy, ProxyScheme},
};

/// 产品根证书（PEM）：当前和备用两张，密钥仪式后填入；根证书私钥离线保管，服务器证书到期换发不用改客户端
const PRODUCT_ROOTS: &[&str] = &[];
/// 开发根证书：私钥在 dreamdesk-gateway 的开发环境里，只有 `dd-managed-dev` 构建认
#[cfg(feature = "dd-managed-dev")]
const DEV_ROOTS: &[&str] = &[include_str!("dev-root-ca.pem")];
#[cfg(not(feature = "dd-managed-dev"))]
const DEV_ROOTS: &[&str] = &[];

fn roots() -> Vec<reqwest::Certificate> {
    PRODUCT_ROOTS
        .iter()
        .chain(DEV_ROOTS)
        .filter_map(|pem| match reqwest::Certificate::from_pem(pem.as_bytes()) {
            Ok(c) => Some(c),
            Err(e) => {
                log::error!("dd-managed: bad built-in root certificate: {e}");
                None
            }
        })
        .collect()
}

/// 配置的代理（和 RustDesk 原来的建法同样的换算）；没配为 `None`，配了但用不了为 `Err`
fn configured_proxy() -> Result<Option<reqwest::Proxy>, String> {
    let Some(conf) = Config::get_socks() else {
        return Ok(None);
    };
    let proxy = Proxy::from_conf(&conf, None).map_err(|e| e.to_string())?;
    let mut p = match &proxy.intercept {
        ProxyScheme::Http { host, .. } => reqwest::Proxy::all(format!("http://{host}")),
        ProxyScheme::Https { host, .. } => reqwest::Proxy::all(format!("https://{host}")),
        ProxyScheme::Socks5 { addr, .. } => reqwest::Proxy::all(format!("socks5://{addr}")),
    }
    .map_err(|e| e.to_string())?;
    if let Some(auth) = proxy.intercept.maybe_auth() {
        if !auth.username().is_empty() && !auth.password().is_empty() {
            p = p.basic_auth(auth.username(), auth.password());
        }
    }
    Ok(Some(p))
}

macro_rules! pinned_client {
    ($Client:ty) => {{
        let pinned = || {
            // 正式版只发 HTTPS（服务器地址也锁成 https 的）；开发测试版要连明文的联调服务
            let mut b = <$Client>::builder()
                .no_proxy()
                .use_rustls_tls()
                .tls_built_in_root_certs(false)
                .https_only(cfg!(not(feature = "dd-managed-dev")));
            for c in roots() {
                b = b.add_root_certificate(c);
            }
            b
        };
        let first = match configured_proxy() {
            Ok(Some(p)) => pinned().proxy(p).build().map_err(|e| e.to_string()),
            Ok(None) => pinned().build().map_err(|e| e.to_string()),
            Err(e) => Err(format!("proxy: {e}")),
        };
        match first {
            Ok(c) => c,
            Err(e) => {
                // 每建一个客户端（心跳每 15 秒）都会走到这里，节流
                hbb_common::throttled_log!(
                    std::time::Duration::from_secs(600),
                    error,
                    "dd-managed: http client: {e}; trying without proxy"
                );
                match pinned().build() {
                    Ok(c) => c,
                    // 连不带代理的 rustls 客户端都建不出来（TLS 后端起不来）。reqwest 这时能给的只有信系统根证书的默认客户端，
                    // 用它就破了证书固定，所以退出（服务会把进程重新拉起）
                    Err(e) => {
                        log::error!("dd-managed: no pinned http client can be built ({e}); exiting");
                        std::process::exit(1)
                    }
                }
            }
        }
    }};
}

pub fn client_async() -> reqwest::Client {
    pinned_client!(reqwest::Client)
}

pub fn client_sync() -> reqwest::blocking::Client {
    pinned_client!(reqwest::blocking::Client)
}
