//! 受管模式的主控端（dreamdesk-gateway 仓库 docs/协议-连接票与设备签名.md 第二、四节）：收到被控端的挑战值后，
//! 凭登录令牌向服务端要这次连接的连接票（`POST /api/dd/ticket`），放进登录请求的 `dd_ticket` 代替口令。
//!
//! 作为 `client` 的子模块编进去（要用 `LoginConfigHandler` 的私有方法），只在 `dd-managed` 下编译。

use super::*;

/// 在 `handle_hash` 开头调用。`None`：这次不走票（没登录，或服务端说目标不是受管工作站），照 RustDesk 原有流程；
/// `Some(keep)`：已经凭票发出登录请求（`true`），或要票失败、已提示用户（`false`，结束这次连接）
pub(super) async fn login_with_ticket(
    lc: &Arc<RwLock<LoginConfigHandler>>,
    hash: &Hash,
    interface: &impl Interface,
    peer: &mut Stream,
) -> Option<bool> {
    let token = LocalConfig::get_option("access_token");
    if token.is_empty() {
        return None;
    }
    // 对端身份验不过时 RustDesk 会退回不加密的连接：这时挑战值可能是中间人从别的连接转来的，要来的票会被它截去用掉，
    // 所以不要票（受管被控端也只在加密连接上认票），照原有流程走
    if !peer.is_secured() {
        log::warn!("dd-managed: connection not encrypted, no ticket requested");
        return None;
    }
    let (target_id, conn_type) = {
        let lc = lc.read().unwrap();
        (lc.id.clone(), conn_type_code(lc.conn_type))
    };
    let url = crate::ui_interface::get_api_server() + "/api/dd/ticket";
    // id / uuid 和界面登录时带的一样（服务端核对令牌是不是这台机器登录拿到的）。取设备号要经 IPC 问服务进程，
    // 那个函数自己起运行时，这里已在异步里，放到阻塞线程上调
    let my_id = hbb_common::tokio::task::spawn_blocking(crate::ui_interface::get_id)
        .await
        .unwrap_or_default();
    let body = serde_json::json!({
        "target_id": target_id,
        "challenge": hash.challenge,
        "conn_type": conn_type,
        "id": my_id,
        "uuid": crate::ui_interface::get_uuid(),
    });
    let header = format!("Authorization: Bearer {token}");
    let fail = |detail: &str| {
        interface.msgbox("error", "Connection Error", "Connection authorization failed", detail);
        Some(false)
    };
    let (status, text) = match crate::post_request_with_status(url, body.to_string(), &header).await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("dd-managed: ticket request for {target_id} failed: {e}");
            return fail("");
        }
    };
    let reply = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
    match status {
        200 => {}
        // 目标不是受管工作站（例如连的是外部的普通 RustDesk）：照原有流程用口令
        404 => {
            log::info!("dd-managed: {target_id} is not a managed workstation, normal login");
            return None;
        }
        _ => {
            let error = reply["error"].as_str().unwrap_or_default();
            log::warn!("dd-managed: ticket for {target_id} refused, status {status}: {error}");
            return fail(error);
        }
    }
    let Some(ticket) = reply["ticket"].as_str().and_then(|t| crate::decode64(t).ok()) else {
        log::warn!("dd-managed: ticket reply for {target_id} has no valid ticket");
        return fail("");
    };
    let mut msg = lc
        .read()
        .unwrap()
        .create_login_msg(String::new(), String::new(), Vec::new());
    if let Some(message::Union::LoginRequest(lr)) = msg.union.as_mut() {
        lr.dd_ticket = ticket.into();
    }
    allow_err!(peer.send(&msg).await);
    Some(true)
}

/// 和被控端核对的连接类型（协议第二节 `conn_type`，对应 `LoginRequest` 的 union）
fn conn_type_code(t: ConnType) -> u8 {
    match t {
        ConnType::FILE_TRANSFER => 1,
        ConnType::PORT_FORWARD | ConnType::RDP => 2,
        ConnType::VIEW_CAMERA => 3,
        ConnType::TERMINAL => 4,
        _ => 0,
    }
}
