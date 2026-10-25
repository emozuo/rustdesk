//! 受管模式的主控端（dreamdesk-gateway 仓库 docs/协议-连接票与设备签名.md 第二、四节）：收到被控端的挑战值后，
//! 向服务端要这次连接的连接票（`POST /api/dd/ticket`；登录了凭令牌以用户身份，没登录凭设备签名以座位身份），
//! 放进登录请求的 `dd_ticket` 代替口令。服务端连不上时可以粘贴离线应急票。
//!
//! 作为 `client` 的子模块编进去（要用 `LoginConfigHandler` 的私有方法），只在 `dd-managed` 下编译。

use super::*;

lazy_static::lazy_static! {
    /// 会话 → 这次连接验证过的对端设备公钥（握手时用注册服务器的签名验过；没验过的不记）。认粘贴的应急票时核对它是不是发给这台的
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
/// `Some(keep)`：已经凭票发出登录请求或弹了口令框等应急票（`true`），或要票被拒、已提示用户（`false`，结束这次连接）
pub(super) async fn login_with_ticket(
    lc: &Arc<RwLock<LoginConfigHandler>>,
    hash: &Hash,
    password_preset: &str,
    interface: &impl Interface,
    peer: &mut Stream,
) -> Option<bool> {
    // 命令行预置的口令（--password）是一张应急票：直接用，不找服务端
    if login_with_pasted_ticket(lc, password_preset, peer).await {
        return Some(true);
    }
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
    // 服务端连不上（或它自己出错）时直接弹口令框，等粘贴 IT 签发的离线应急票（login_with_pasted_ticket）。不走 RustDesk 原有流程：
    // 它会先试存下的口令，受管被控端拒绝后就断开，用户见不到口令框。
    // kiosk 前没人能粘贴应急票：报错，由状态页过一会儿重新问绑定再连
    let no_server = || {
        if is_kiosk() {
            return fail("Cannot reach the server");
        }
        interface.msgbox("input-password", "Password Required", "", "");
        Some(true)
    };
    let (status, text) = match crate::post_request_with_status(url, body.to_string(), &header).await {
        Ok(r) if r.0 < 500 => r,
        Ok((status, _)) => {
            log::warn!("dd-managed: ticket request for {target_id}: server error {status}");
            return no_server();
        }
        Err(e) => {
            log::warn!("dd-managed: ticket request for {target_id} failed: {e}");
            return no_server();
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

/// 本进程是 kiosk 瘦客户机的界面进程（`--kiosk`，远程窗口和它同一个进程）：前面没人能输口令、粘贴应急票
fn is_kiosk() -> bool {
    std::env::args().any(|a| a == "--kiosk")
}

/// 在 `handle_login_from_ui` 开头调用：口令框里粘贴的是一张应急票（IT 用离线应急钥签的，服务端连不上时用）就凭它登录，
/// 返回 `true`；不是就返回 `false`，照口令处理。真伪和是否用过由被控端验
pub(super) async fn login_with_pasted_ticket(
    lc: &Arc<RwLock<LoginConfigHandler>>,
    pasted: &str,
    peer: &mut Stream,
) -> bool {
    // base64 里没有空格和换行：命令行 --password 经链接传给界面时 `+` 会变成空格，复制粘贴可能带进换行
    let pasted: String = pasted.trim().chars().filter(|c| *c != '\n' && *c != '\r').map(|c| if c == ' ' { '+' } else { c }).collect();
    let Some(ticket) = crate::decode64(&pasted).ok().filter(|b| {
        ddpro::ticket::parse_unverified(b).is_ok_and(|t| {
            t.sub_kind == ddpro::ticket::SubKind::Emergency && t.kid.starts_with(ddpro::ticket::EMERGENCY_KID_PREFIX)
        })
    }) else {
        return false;
    };
    // 应急票谁先用算谁的：不是发给这次连上的这台（设备号输错、被骗去连了别人的机器）就不发，当口令处理（只发散列）
    if !for_connected_peer(lc, &ticket) {
        return false;
    }
    // 应急票不绑这次连接的挑战值，明文发出去会被中间人截去先用掉（只能用一次）。不发票、当口令处理：
    // 口令只发散列，从中还原不出票，受管被控端照样拒绝
    if !peer.is_secured() {
        log::warn!("dd-managed: connection not encrypted, emergency ticket not sent");
        return false;
    }
    log::warn!("dd-managed: logging in with a pasted emergency ticket");
    send_ticket_login(lc, ticket, peer).await;
    true
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
