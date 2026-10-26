//! 受管模式的主控端（dreamdesk-gateway 仓库 docs/协议-连接票与设备签名.md 第二、四节）：收到被控端的挑战值后，
//! 向服务端要这次连接的连接票（`POST /api/dd/ticket`；登录了凭令牌以用户身份，没登录凭设备签名以座位身份），
//! 放进登录请求的 `dd_ticket` 代替口令。
//!
//! 作为 `client` 的子模块编进去（要用 `LoginConfigHandler` 的私有方法），只在 `dd-managed` 下编译。

use super::*;

lazy_static::lazy_static! {
    /// 会话 → 这次连接验证过的对端设备公钥（握手时用注册服务器的签名验过；没验过的不记）。交票前核对票是不是发给这台的
    static ref PEER_PK: Mutex<HashMap<u64, Vec<u8>>> = Default::default();
}

/// 连上之后调用（远程桌面在 io_loop、端口转发 / RDP 在 port_forward；`pk` 是 `Client::start` 返回的、验证过的对端公钥）
pub(crate) fn set_peer_pk(lc: &Arc<RwLock<LoginConfigHandler>>, pk: Option<Vec<u8>>) {
    let session_id = lc.read().unwrap().session_id;
    let mut map = PEER_PK.lock().unwrap();
    match pk {
        Some(pk) => map.insert(session_id, pk),
        None => map.remove(&session_id),
    };
}

/// 在 `handle_hash` 里、RustDesk 试口令之前调用。登录了以用户身份要票，没登录以座位身份要票。
/// `None`：这次不走票（连接没加密、服务端说目标不是受管工作站、或不是这台的座位），照 RustDesk 原有流程；
/// `Some(keep)`：已经凭票发出登录请求（`true`），或要不到票、已提示用户（`false`，结束这次连接）
pub(super) async fn login_with_ticket(
    lc: &Arc<RwLock<LoginConfigHandler>>,
    hash: &Hash,
    interface: &impl Interface,
    peer: &mut Stream,
) -> Option<bool> {
    // 对端身份验不过时 RustDesk 会退回不加密的连接：这时挑战值可能是中间人从别的连接转来的，要来的票会被它截去用掉，
    // 所以不要票（受管被控端也只在加密连接上认票），照原有流程走
    if !peer.is_secured() {
        log::warn!("dd-managed: connection not encrypted, no ticket requested");
        return None;
    }
    // 没登录就以座位身份要票（kiosk 瘦客户机不登录）：请求不带令牌、由 device_sig 用设备密钥签名，服务端按公钥认座位、
    // 只发它绑定的那台的票
    let token = LocalConfig::get_option("access_token");
    let seat = token.is_empty();
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
    let header = if seat { String::new() } else { format!("Authorization: Bearer {token}") };
    // 服务端给的原因是给人看的整句话，放在提示正文（第四个参数是链接，放那儿只会多一个跳转按钮）。
    // 标题不用「Connection Error」：RustDesk 按那个标题加正文里的英文词判断要不要自动重连，中文原因会被当成可重连，
    // 被拒之后一直重连、一直要票。kiosk 下由状态页接手（flutter/lib/utils/dd_kiosk.dart）
    let fail = |reason: &str| {
        let text = if reason.is_empty() { "Connection authorization failed" } else { reason };
        interface.msgbox("error", "Connection authorization failed", text, "");
        Some(false)
    };
    // 服务端连不上（或它自己出错）就报错，不走 RustDesk 原有流程：它会先试存下的口令，受管被控端拒绝后报的是
    // 「只接受服务器授权的连接」，看不出真正的原因是服务端连不上
    let (status, text) = match crate::post_request_with_status(url, body.to_string(), &header).await {
        Ok(r) if r.0 < 500 => r,
        Ok((status, _)) => {
            log::warn!("dd-managed: ticket request for {target_id}: server error {status}");
            return fail("Cannot reach the server");
        }
        Err(e) => {
            log::warn!("dd-managed: ticket request for {target_id} failed: {e}");
            return fail("Cannot reach the server");
        }
    };
    let reply = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
    match status {
        200 => {}
        // 目标不是受管工作站（例如连的是外部的普通 RustDesk），或没登录的这台不是座位：照原有流程用口令
        // kiosk 例外：原有流程会弹口令框，kiosk 前没人能输入，报出服务端的原因
        404 if is_kiosk() => {
            let error = reply["error"].as_str().unwrap_or_default();
            log::warn!("dd-managed: kiosk: {target_id} is not a managed workstation or this is not a seat: {error}");
            return fail(error);
        }
        404 => {
            log::info!("dd-managed: {target_id} is not a managed workstation, normal login");
            return None;
        }
        // 其余 4xx：用户没授权、或座位被拒（没确认、没绑这台、疑似克隆、签名不对……；不是座位的设备服务端回 404，见上）。
        // 显示服务端给的原因、结束这次连接——退回原有流程会先试存下的口令，被受管被控端拒绝后断开，用户见不到原因
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
    // 服务端签的票也核对一遍是不是发给这次连上的这台：注册服务器的钥匙被换掉、中间人把真目标的挑战值转过来时，票不交给它
    if !for_connected_peer(lc, &ticket) {
        return fail("");
    }
    send_ticket_login(lc, ticket, peer).await;
    Some(true)
}

/// 本进程是 kiosk 瘦客户机的界面进程（`--kiosk`，远程窗口和它同一个进程）：前面没人能输口令
fn is_kiosk() -> bool {
    std::env::args().any(|a| a == "--kiosk")
}

/// 票里的目标公钥是不是这次连接握手时验证过的对端公钥（没验证过的连接一律不是）
fn for_connected_peer(lc: &Arc<RwLock<LoginConfigHandler>>, ticket: &[u8]) -> bool {
    let Ok(t) = ddpro::ticket::parse_unverified(ticket) else {
        return false;
    };
    let session_id = lc.read().unwrap().session_id;
    let ok = PEER_PK.lock().unwrap().get(&session_id).map(|pk| pk.as_slice()) == Some(&t.target_pk[..]);
    if !ok {
        log::warn!("dd-managed: the ticket is for device {}, not the one connected; not sent", t.target_id);
    }
    ok
}

async fn send_ticket_login(lc: &Arc<RwLock<LoginConfigHandler>>, ticket: Vec<u8>, peer: &mut Stream) {
    let mut msg = lc
        .read()
        .unwrap()
        .create_login_msg(String::new(), String::new(), Vec::new());
    if let Some(message::Union::LoginRequest(lr)) = msg.union.as_mut() {
        lr.dd_ticket = ticket.into();
    }
    allow_err!(peer.send(&msg).await);
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
