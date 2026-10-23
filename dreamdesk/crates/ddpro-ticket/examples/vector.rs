//! 固定测试向量：打印连接票和设备签名的字节，和服务端（Go）按同一组取值算出的结果逐字节比对；
//! 再把验签和各种该拒绝的情况真实跑一遍，打印通过 / 失败。
//!
//!   cargo run -p ddpro-ticket --example vector   （在 dreamdesk/crates/ddpro-ticket 下）

use ddpro_ticket::{device_signature_headers, keypair_from_seed, verify, Error, SubKind, Ticket};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn main() {
    // 取值和 dreamdesk-gateway 的 docs/测试向量.md 一致
    let (srv_sk, srv_pk) = keypair_from_seed(&[1u8; 32]);
    let (dev_sk, dev_pk) = keypair_from_seed(&[2u8; 32]);
    let mut target_pk = [0u8; 32];
    target_pk.copy_from_slice(&dev_pk);
    let t = Ticket {
        kid: "dev1".to_owned(),
        sub_kind: SubKind::User,
        sub: "6f1c2a3b-0000-4000-8000-000000000001".to_owned(),
        sub_name: "张三".to_owned(),
        via: String::new(),
        target_id: "123456789".to_owned(),
        target_pk,
        challenge: "abc123".to_owned(),
        conn_type: 0,
        perms: u32::MAX,
        iat: 1_792_730_000,
        exp: 1_792_730_120,
        jti: [7u8; 16],
    };
    let ticket = t.sign(&srv_sk).expect("sign");
    println!("server pk      {}", hex(&srv_pk));
    println!("device pk      {}", hex(&dev_pk));
    println!("ticket payload {}", hex(&t.payload()));
    println!("ticket sig     {}", hex(&ticket[ticket.len() - 64..]));
    let body = br#"{"id":"123456789"}"#;
    for (k, v) in device_signature_headers(&dev_sk, &dev_pk, "POST", "/api/heartbeat", "", 1_792_730_000, body).expect("device sig") {
        println!("{k}: {v}");
    }

    let trusted = [("dev1", srv_pk.as_slice())];
    let now = 1_792_730_050;
    let mut ok = true;
    let mut expect = |name: &str, got: Result<(), Error>, want: Result<(), Error>| {
        let pass = got == want;
        ok &= pass;
        println!("{} {name}: {got:?}", if pass { "PASS" } else { "FAIL" });
    };
    let v = verify(&ticket, &trusted);
    expect("verify", v.as_ref().map(|_| ()).map_err(Clone::clone), Ok(()));
    let v = v.expect("verified");
    expect("check ok", v.check(&dev_pk, "abc123", 0, now), Ok(()));
    expect("other challenge", v.check(&dev_pk, "zzz999", 0, now), Err(Error::WrongChallenge));
    expect("other device", v.check(&[9u8; 32], "abc123", 0, now), Err(Error::WrongTarget));
    expect("other conn type", v.check(&dev_pk, "abc123", 1, now), Err(Error::WrongConnType));
    expect("expired", v.check(&dev_pk, "abc123", 0, t.exp + 61), Err(Error::Expired));
    expect("not yet valid", v.check(&dev_pk, "abc123", 0, t.iat - 61), Err(Error::NotYetValid));
    let mut bad = ticket.clone();
    bad[10] ^= 1;
    expect("tampered", verify(&bad, &trusted).map(|_| ()), Err(Error::BadSignature));
    expect(
        "unknown key",
        verify(&ticket, &[("other", srv_pk.as_slice())]).map(|_| ()),
        Err(Error::UnknownKey("dev1".to_owned())),
    );
    // 应急：应急钥签的应急票不核对挑战值；普通钥签的「应急票」、应急钥签的普通票都不认
    let (emg_sk, emg_pk) = keypair_from_seed(&[3u8; 32]);
    let mut e = t.clone();
    e.kid = "emg1".to_owned();
    e.sub_kind = SubKind::Emergency;
    e.challenge = String::new();
    e.exp = e.iat + 4 * 3600;
    let ev = verify(&e.sign(&emg_sk).expect("sign"), &[("emg1", emg_pk.as_slice())]).expect("emergency verify");
    expect("emergency ok", ev.check(&dev_pk, "anything", 0, e.iat + 3600), Ok(()));
    let mut fake = e.clone();
    fake.kid = "t2026a".to_owned();
    expect("emergency with normal key", fake.check(&dev_pk, "", 0, e.iat), Err(Error::EmergencyMismatch));
    let mut user = t.clone();
    user.kid = "emg1".to_owned();
    expect("normal ticket with emergency key", user.check(&dev_pk, "abc123", 0, now), Err(Error::EmergencyMismatch));
    println!("{}", if ok { "ALL PASS" } else { "SOME FAILED" });
    std::process::exit(if ok { 0 } else { 1 });
}
