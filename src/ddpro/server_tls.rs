//! 受管模式的服务器证书校验（dreamdesk-gateway 仓库 docs/S1-方案.md 第五节第 9 条）：RustDesk 的网络请求出错时会改成
//! 「接受无效证书」重试并记住，登录口令、令牌、票、心跳下发的设置都经过它。受管构建里所有 HTTP 客户端只信任预置的
//! DreamDesk 根证书（产品服务器的证书由它签发），不信任系统和内置的根证书，也不接受无效证书。
//! 产品是内网专用，受管构建里没有别的 HTTPS 目的地，所以统一挂在 RustDesk 建 HTTP 客户端的两个函数里。

/// 产品根证书（PEM）。密钥仪式后填入；根证书私钥离线保管，服务器证书到期换发不用改客户端
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
                hbb_common::log::error!("dd-managed: bad built-in root certificate: {e}");
                None
            }
        })
        .collect()
}

pub fn pin_async(mut b: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    b = b.tls_built_in_root_certs(false);
    for c in roots() {
        b = b.add_root_certificate(c);
    }
    b
}

pub fn pin_sync(mut b: reqwest::blocking::ClientBuilder) -> reqwest::blocking::ClientBuilder {
    b = b.tls_built_in_root_certs(false);
    for c in roots() {
        b = b.add_root_certificate(c);
    }
    b
}
