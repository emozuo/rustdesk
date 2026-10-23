//! 受管模式（dreamdesk-gateway 仓库 docs/协议-连接票与设备签名.md 第二、六节）：被控端只认服务端签发的连接票和离线应急票，
//! RustDesk 原有的口令、点击接受、近期会话复用、切换方向、二次验证、登录界面口令、终端系统账号登录一律不认。
//!
//! 作为 `server::connection` 的子模块编进去（要用 `Connection` 的私有字段和方法），只在 `dd-managed` 下编译；
//! 不开时被控端照旧走 RustDesk 原有认证。

use super::*;
use ddpro::ticket::{self, SubKind, Ticket};
use std::io::Write;

/// 编进来就受管。钩子写成 `if MANAGED { return ... }`，是为了让钩子后面 RustDesk 原有的代码不被报「执行不到」
pub(super) const MANAGED: bool = true;

/// 产品预置的受信任公钥（钥匙编号, Ed25519 公钥十六进制）。密钥仪式后填入当前钥、备用钥和应急钥
const PRODUCT_KEYS: &[(&str, &str)] = &[];
/// 开发测试钥：私钥在 dreamdesk-gateway 的开发环境里，只有 `dd-managed-dev` 构建认，产品构建不带
#[cfg(feature = "dd-managed-dev")]
const DEV_KEYS: &[(&str, &str)] = &[
    ("devsrv1", "d6b608d34808ded7ecad1dbc57017723211fd45ec52a82bc5b4b33425de56252"),
    ("emgdev1", "0df235d9b9be02314edf20e72a3c724392f9ff5d3316f34242f44f518d802938"),
];
#[cfg(not(feature = "dd-managed-dev"))]
const DEV_KEYS: &[(&str, &str)] = &[];

/// 用过的应急票编号，一行一个（应急票不绑挑战值，靠它保证只能用一次）
const EMERGENCY_USED_FILE: &str = "dd_emergency_used";

const LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

const MSG_TICKET_REQUIRED: &str = "This device only accepts connections authorized by the server";
const MSG_TICKET_REJECTED: &str = "Connection authorization rejected";

lazy_static::lazy_static! {
    static ref EMERGENCY_LOCK: Mutex<()> = Default::default();
}

impl Connection {
    /// 受管时登录请求在这里走完：验票通过就接上 RustDesk 原有的认证成功后续（发登录回复、起连接管理），否则拒绝并断开。
    /// 登录请求取 `self.lr`（RustDesk 收到时已存下）。返回值同 `on_message`：是否保持连接
    pub(super) async fn dd_managed_login(&mut self) -> bool {
        let lr = self.lr.clone();
        // 被拒的登录由对端想发多少就发多少，日志要节流
        // 空口令、不带票：主控端要不到票（服务端连不上、没登录）时照 RustDesk 原样先发一个空口令的登录、弹口令框等输入。
        // 受管时口令框是粘贴离线应急票的地方，所以不断开，回「要口令」等它再发（未认证连接照常有超时）
        if lr.dd_ticket.is_empty() && lr.password.is_empty() {
            self.send_login_error(crate::client::LOGIN_MSG_PASSWORD_EMPTY).await;
            return true;
        }
        if lr.dd_ticket.is_empty() {
            hbb_common::throttled_log!(LOG_INTERVAL, warn, "dd-managed: login from {} without ticket refused", lr.my_id);
            self.send_login_error(MSG_TICKET_REQUIRED).await;
            sleep(1.).await;
            return false;
        }
        let mut checked = self.dd_check_ticket(&lr);
        // 应急票记「已用」要写盘（先写盘再放行），放到阻塞线程上做
        if let Some(t) = checked.as_ref().ok().filter(|t| t.sub_kind == SubKind::Emergency) {
            let jti = t.jti_hex();
            if let Err(e) = hbb_common::tokio::task::spawn_blocking(move || claim_emergency(&jti))
                .await
                .unwrap_or_else(|e| Err(e.to_string()))
            {
                checked = Err(e);
            }
        }
        let t = match checked {
            Ok(t) => t,
            Err(e) => {
                hbb_common::throttled_log!(LOG_INTERVAL, warn, "dd-managed: ticket from {} refused: {e}", lr.my_id);
                self.send_login_error(MSG_TICKET_REJECTED).await;
                sleep(1.).await;
                return false;
            }
        };
        let jti = t.jti_hex();
        log::info!(
            "dd-managed: ticket {jti} accepted, {:?} {} ({}), from {}",
            t.sub_kind,
            t.sub_name,
            t.sub,
            lr.my_id
        );
        if t.sub_kind == SubKind::Emergency {
            log::warn!("dd-managed: EMERGENCY ticket {jti} ({}) used by {}", t.sub, lr.my_id);
            let rec = json!({
                "jti": jti,
                "kid": t.kid,
                "sub": t.sub,
                "sub_name": t.sub_name,
                "peer_id": lr.my_id,
                "peer_name": lr.my_name,
                "conn_type": t.conn_type,
                "at": hbb_common::get_time() / 1000,
                "exp": t.exp,
            });
            // 记待补传也要写盘
            if let Err(e) =
                hbb_common::tokio::task::spawn_blocking(move || crate::ddpro::emergency_audit::record(rec)).await
            {
                log::error!("dd-managed: emergency audit not recorded: {e}");
            }
        }
        let id = self.inner.id();
        // 连着期间一直记着（连接审计和每拍心跳都带），连接关闭时在 ddpro::server::on_conn_closed 里删
        crate::ddpro::ticket_conns::insert(id, jti);
        // 票由服务端在 AD 登录、授权检查之后才签发，二次验证不再单独要
        self.require_2fa = None;
        let keep = self.send_logon_response_and_keep_alive().await;
        if !keep || !self.authorized {
            crate::ddpro::ticket_conns::remove(id);
        }
        if keep {
            self.try_start_cm(lr.my_id, lr.my_name, self.authorized);
        }
        keep
    }

    fn dd_check_ticket(&self, lr: &LoginRequest) -> Result<Ticket, String> {
        if !self.stream.is_secured() {
            return Err("connection not encrypted".to_owned());
        }
        if !lr.password.is_empty() || !lr.os_login.username.is_empty() {
            return Err("password / OS login sent along with the ticket".to_owned());
        }
        let keys = trusted_keys();
        let trusted: Vec<(&str, &[u8])> = keys.iter().map(|(k, pk)| (*k, pk.as_slice())).collect();
        let t = ticket::verify(&lr.dd_ticket, &trusted).map_err(|e| e.to_string())?;
        let conn_type = match lr.union {
            None => 0,
            Some(login_request::Union::FileTransfer(_)) => 1,
            Some(login_request::Union::PortForward(_)) => 2,
            Some(login_request::Union::ViewCamera(_)) => 3,
            Some(login_request::Union::Terminal(_)) => 4,
            Some(_) => return Err("unknown connection type".to_owned()),
        };
        let (_, my_pk) = Config::get_key_pair();
        t.check(&my_pk, &self.hash.challenge, conn_type, hbb_common::get_time() / 1000)
            .map_err(|e| e.to_string())?;
        if t.target_id != Config::get_id() {
            // 以公钥为准：设备号可能被 hbbs 的冲突处理改掉
            log::warn!("dd-managed: ticket names device {}, this device is {}", t.target_id, Config::get_id());
        }
        Ok(t)
    }
}

/// 认证成功发连接审计时调用：凭票登录的连接带上票编号
pub(super) fn add_audit_jti(conn_id: i32, audit: &mut Value) {
    if let Some(jti) = crate::ddpro::ticket_conns::get(conn_id) {
        audit["dd_ticket_jti"] = json!(jti);
    }
}

fn trusted_keys() -> Vec<(&'static str, Vec<u8>)> {
    PRODUCT_KEYS
        .iter()
        .chain(DEV_KEYS)
        .filter_map(|(kid, pk)| match hex::decode(pk) {
            Ok(pk) if pk.len() == 32 => Some((*kid, pk)),
            _ => {
                log::error!("dd-managed: bad built-in key {kid}");
                None
            }
        })
        .collect()
}

/// 应急票只能用一次：没用过就先记进本机文件（落盘后）再放行
fn claim_emergency(jti: &str) -> Result<(), String> {
    let _guard = EMERGENCY_LOCK.lock().unwrap();
    let path = Config::path(EMERGENCY_USED_FILE);
    let used = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    if used.lines().any(|l| l.trim() == jti) {
        return Err(format!("emergency ticket {jti} already used"));
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    writeln!(f, "{jti}")
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("write {}: {e}", path.display()))
}
