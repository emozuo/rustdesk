//! 连接票与设备签名，按 dreamdesk-gateway 仓库的 `docs/协议-连接票与设备签名.md`（v1）实现；改格式要两边一起改，
//! 并用两边共同的固定测试向量（`docs/测试向量.md`）核对。
//!
//! - 连接票：服务端用 Ed25519 签发，主控端放在登录消息里带给被控端，被控端用预置的公钥验（[`verify`] + [`Ticket::check`]）。
//! - 设备签名：设备用自己的 Ed25519 私钥（RustDesk 的设备密钥对）给发往服务端的请求签名（[`device_signature_headers`]）。

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sha2::{Digest, Sha256};
use sodiumoxide::crypto::sign;

/// 票签名的域隔离前缀（`"DDTICKET-v1\0"`）
const TICKET_DOMAIN: &[u8] = b"DDTICKET-v1\0";
/// 设备签名的域隔离前缀（`"DDDEV-v1\0"`）
const DEVICE_DOMAIN: &[u8] = b"DDDEV-v1\0";
const VERSION: u8 = 1;
const SIG_LEN: usize = 64;
/// 票总长上限
pub const MAX_TICKET_LEN: usize = 1024;
/// 被控端容忍的时钟差（秒）
pub const CLOCK_SKEW: i64 = 60;
/// 有效期上限（exp − iat，秒）
pub const MAX_LIFETIME: i64 = 120;

/// 票的主体：只有登录了的人（用户 10-26 定所有客户端都必须登录）。2（座位）、3（应急）已撤掉，不再复用，
/// 收到按格式错误拒
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubKind {
    User = 1,
}

impl SubKind {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::User),
            _ => None,
        }
    }
}

/// 解出来的票（字段含义见协议文档第二节）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub kid: String,
    pub sub_kind: SubKind,
    pub sub: String,
    pub sub_name: String,
    pub via: String,
    pub target_id: String,
    pub target_pk: [u8; 32],
    pub challenge: String,
    pub conn_type: u8,
    pub perms: u32,
    pub iat: i64,
    pub exp: i64,
    pub jti: [u8; 16],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// 长度、版本或字段格式不对
    Malformed(&'static str),
    /// 钥匙编号不在受信任的公钥里
    UnknownKey(String),
    BadSignature,
    /// 票不是发给这台机器的（设备公钥对不上）
    WrongTarget,
    /// 不是这次连接的挑战值
    WrongChallenge,
    /// 连接类型和登录请求的不一致
    WrongConnType,
    Expired,
    /// 签发时间在未来（超出时钟容忍）
    NotYetValid,
    /// 有效期比协议允许的长：签发方出错或钥匙被滥用
    LifetimeTooLong,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Malformed(what) => write!(f, "malformed ticket ({what})"),
            Error::UnknownKey(kid) => write!(f, "ticket signed by an unknown key {kid:?}"),
            Error::BadSignature => write!(f, "bad ticket signature"),
            Error::WrongTarget => write!(f, "the ticket is for another device"),
            Error::WrongChallenge => write!(f, "the ticket is for another connection"),
            Error::WrongConnType => write!(f, "the ticket is for another kind of connection"),
            Error::Expired => write!(f, "the ticket has expired"),
            Error::NotYetValid => write!(f, "the ticket is not valid yet (clock difference too large)"),
            Error::LifetimeTooLong => write!(f, "the ticket is valid for longer than allowed"),
        }
    }
}

impl std::error::Error for Error {}

fn put_var(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u16).to_be_bytes());
    out.extend_from_slice(b);
}

impl Ticket {
    /// 载荷字节（不含签名）
    pub fn payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        out.push(VERSION);
        out.push(self.kid.len() as u8);
        out.extend_from_slice(self.kid.as_bytes());
        out.push(self.sub_kind as u8);
        put_var(&mut out, self.sub.as_bytes());
        put_var(&mut out, self.sub_name.as_bytes());
        put_var(&mut out, self.via.as_bytes());
        put_var(&mut out, self.target_id.as_bytes());
        out.extend_from_slice(&self.target_pk);
        put_var(&mut out, self.challenge.as_bytes());
        out.push(self.conn_type);
        out.extend_from_slice(&self.perms.to_be_bytes());
        out.extend_from_slice(&self.iat.to_be_bytes());
        out.extend_from_slice(&self.exp.to_be_bytes());
        out.extend_from_slice(&self.jti);
        out
    }

    /// 用 `sk`（sodium 的 64 字节私钥）签名，返回整张票（开发测试和测试向量用；产品里只有服务端签）
    pub fn sign(&self, sk: &[u8]) -> Result<Vec<u8>, Error> {
        let sk = sign::SecretKey::from_slice(sk).ok_or(Error::Malformed("secret key"))?;
        let payload = self.payload();
        let sig = sign::sign_detached(&[TICKET_DOMAIN, &payload].concat(), &sk);
        let mut out = payload;
        out.extend_from_slice(sig.as_ref());
        Ok(out)
    }

    /// 被控端核对票的用途：发给本机（`my_pk`）、这次连接（`challenge`、`conn_type`）、在有效期内（`now`，Unix 秒）
    pub fn check(&self, my_pk: &[u8], challenge: &str, conn_type: u8, now: i64) -> Result<(), Error> {
        if self.target_pk[..] != *my_pk {
            return Err(Error::WrongTarget);
        }
        if self.challenge != challenge {
            return Err(Error::WrongChallenge);
        }
        if self.conn_type != conn_type {
            return Err(Error::WrongConnType);
        }
        if self.exp.saturating_sub(self.iat) > MAX_LIFETIME {
            return Err(Error::LifetimeTooLong);
        }
        if now > self.exp.saturating_add(CLOCK_SKEW) {
            return Err(Error::Expired);
        }
        if self.iat > now.saturating_add(CLOCK_SKEW) {
            return Err(Error::NotYetValid);
        }
        Ok(())
    }

    /// 票编号的十六进制（审计里报给服务端的 `dd_ticket_jti`）
    pub fn jti_hex(&self) -> String {
        self.jti.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// 逐字段读载荷
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], Error> {
        if self.0.len() < n {
            return Err(Error::Malformed(what));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, Error> {
        Ok(self.take(1, what)?[0])
    }
    fn var(&mut self, what: &'static str) -> Result<&'a [u8], Error> {
        let n = u16::from_be_bytes([self.u8(what)?, self.u8(what)?]) as usize;
        self.take(n, what)
    }
    fn string(&mut self, what: &'static str) -> Result<String, Error> {
        String::from_utf8(self.var(what)?.to_vec()).map_err(|_| Error::Malformed(what))
    }
    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], Error> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N, what)?);
        Ok(a)
    }
}

/// 解析并验签：`trusted` 是本机受信任的（钥匙编号, 32 字节公钥）。通过后还要调用方用 [`Ticket::check`] 核对用途
pub fn verify(ticket: &[u8], trusted: &[(&str, &[u8])]) -> Result<Ticket, Error> {
    let (t, payload, sig) = split_parse(ticket)?;
    let (_, pk) = trusted.iter().find(|(k, _)| *k == t.kid).ok_or_else(|| Error::UnknownKey(t.kid.clone()))?;
    let pk = sign::PublicKey::from_slice(pk).ok_or(Error::Malformed("trusted public key"))?;
    let sig = sign::Signature::from_bytes(sig).map_err(|_| Error::BadSignature)?;
    if !sign::verify_detached(&sig, &[TICKET_DOMAIN, payload].concat(), &pk) {
        return Err(Error::BadSignature);
    }
    Ok(t)
}

/// 只解析、不验签：主控端核对服务端给的票是不是发给这次连上的那台，真伪由被控端验
pub fn parse_unverified(ticket: &[u8]) -> Result<Ticket, Error> {
    split_parse(ticket).map(|(t, _, _)| t)
}

/// 拆成（解出的票, 载荷, 签名）
fn split_parse(ticket: &[u8]) -> Result<(Ticket, &[u8], &[u8]), Error> {
    if ticket.len() > MAX_TICKET_LEN || ticket.len() <= SIG_LEN {
        return Err(Error::Malformed("length"));
    }
    let (payload, sig) = ticket.split_at(ticket.len() - SIG_LEN);
    let mut r = Reader(payload);
    if r.u8("version")? != VERSION {
        return Err(Error::Malformed("version"));
    }
    let kid_len = r.u8("kid")? as usize;
    let kid = String::from_utf8(r.take(kid_len, "kid")?.to_vec()).map_err(|_| Error::Malformed("kid"))?;
    let sub_kind = SubKind::from_u8(r.u8("sub_kind")?).ok_or(Error::Malformed("sub_kind"))?;
    let t = Ticket {
        kid,
        sub_kind,
        sub: r.string("sub")?,
        sub_name: r.string("sub_name")?,
        via: r.string("via")?,
        target_id: r.string("target_id")?,
        target_pk: r.array::<32>("target_pk")?,
        challenge: r.string("challenge")?,
        conn_type: r.u8("conn_type")?,
        perms: u32::from_be_bytes(r.array::<4>("perms")?),
        iat: i64::from_be_bytes(r.array::<8>("iat")?),
        exp: i64::from_be_bytes(r.array::<8>("exp")?),
        jti: r.array::<16>("jti")?,
    };
    if !r.0.is_empty() {
        return Err(Error::Malformed("trailing bytes"));
    }
    Ok((t, payload, sig))
}

/// 设备签名要签的内容：`"DDDEV-v1\0"` ‖ 方法 ‖ \n ‖ 路径 ‖ \n ‖ 查询串 ‖ \n ‖ 时间 ‖ \n ‖ SHA256(请求体)
fn device_message(method: &str, path: &str, query: &str, ts: i64, body: &[u8]) -> Vec<u8> {
    let mut m = DEVICE_DOMAIN.to_vec();
    for part in [method.as_bytes(), b"\n", path.as_bytes(), b"\n", query.as_bytes(), b"\n", ts.to_string().as_bytes(), b"\n"] {
        m.extend_from_slice(part);
    }
    m.extend_from_slice(&Sha256::digest(body));
    m
}

/// 设备签名的三个请求头（`X-DD-Device-Pk` / `-Ts` / `-Sig`），每行一个 `名字: 值`。`sk` 是 sodium 的 64 字节私钥，`pk` 是 32 字节公钥
pub fn device_signature_headers(
    sk: &[u8],
    pk: &[u8],
    method: &str,
    path: &str,
    query: &str,
    ts: i64,
    body: &[u8],
) -> Result<Vec<(String, String)>, Error> {
    let sk = sign::SecretKey::from_slice(sk).ok_or(Error::Malformed("secret key"))?;
    let sig = sign::sign_detached(&device_message(method, path, query, ts, body), &sk);
    Ok(vec![
        ("X-DD-Device-Pk".to_owned(), B64.encode(pk)),
        ("X-DD-Device-Ts".to_owned(), ts.to_string()),
        ("X-DD-Device-Sig".to_owned(), B64.encode(sig.as_ref())),
    ])
}

/// 由 32 字节种子得到（64 字节私钥, 32 字节公钥）——测试向量和开发测试用
pub fn keypair_from_seed(seed: &[u8; 32]) -> (Vec<u8>, Vec<u8>) {
    let (pk, sk) = sign::keypair_from_seed(&sign::Seed(*seed));
    (sk.as_ref().to_vec(), pk.as_ref().to_vec())
}
