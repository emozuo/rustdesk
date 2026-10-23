//! 被控端验票用例对齐：读 dreamdesk-gateway 的 tools/acceptance/testdata/host-verify-cases.json（服务端独立实现生成），
//! 按被控端 src/ddpro/managed.rs 的顺序逐条跑（连接没加密先拒 → 验签 → 核对用途 → 应急票查是否用过），和期望的结论比对。
//!
//!   cargo run -p ddpro-ticket --example host_cases -- <host-verify-cases.json>

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ddpro_ticket::{verify, Error, SubKind};

fn reason(e: &Error) -> &'static str {
    match e {
        Error::Malformed(_) => "bad_format",
        Error::UnknownKey(_) => "unknown_kid",
        Error::BadSignature => "bad_sig",
        Error::WrongTarget => "wrong_target",
        Error::WrongChallenge => "wrong_challenge",
        Error::WrongConnType => "wrong_conn_type",
        Error::Expired => "expired",
        Error::NotYetValid => "not_yet_valid",
        Error::EmergencyMismatch => "emergency_mismatch",
        Error::LifetimeTooLong => "lifetime_too_long",
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: host_cases <host-verify-cases.json>");
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read cases")).expect("parse cases");
    let keys: Vec<(String, Vec<u8>)> = doc["trusted_keys_b64"]
        .as_object()
        .expect("trusted_keys_b64")
        .iter()
        .map(|(k, v)| (k.clone(), B64.decode(v.as_str().unwrap_or_default()).expect("key b64")))
        .collect();
    let trusted: Vec<(&str, &[u8])> = keys.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect();
    let (mut pass, mut fail) = (0, 0);
    for c in doc["cases"].as_array().expect("cases") {
        let s = |k: &str| c[k].as_str().unwrap_or_default().to_owned();
        let got: Result<(), &str> = (|| {
            if !c["encrypted"].as_bool().unwrap_or(false) {
                return Err("not_encrypted");
            }
            let ticket = B64.decode(s("ticket_b64")).map_err(|_| "bad_format")?;
            let t = verify(&ticket, &trusted).map_err(|e| reason(&e))?;
            let my_pk = B64.decode(s("self_pk_b64")).map_err(|_| "bad_format")?;
            let conn_type = c["conn_type"].as_u64().unwrap_or(0) as u8;
            t.check(&my_pk, &s("challenge"), conn_type, c["now"].as_i64().unwrap_or(0))
                .map_err(|e| reason(&e))?;
            let used = c["used_emergency_jti_hex"].as_array().cloned().unwrap_or_default();
            if t.sub_kind == SubKind::Emergency && used.iter().any(|u| u.as_str() == Some(&t.jti_hex())) {
                return Err("emergency_reused");
            }
            Ok(())
        })();
        let want = if s("expect") == "accept" { Ok(()) } else { Err(s("reason")) };
        let ok = got.map_err(str::to_owned) == want;
        if ok { pass += 1 } else { fail += 1 }
        println!(
            "{} {} — want {:?}, got {:?}",
            if ok { "PASS" } else { "FAIL" },
            s("name"),
            want,
            got
        );
    }
    println!("{pass} passed, {fail} failed");
    std::process::exit(if fail == 0 { 0 } else { 1 });
}
