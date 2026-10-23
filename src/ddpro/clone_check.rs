//! 受管模式的克隆自查（dreamdesk-gateway 仓库 docs/S1-方案.md 第二节「克隆三道防线」第二道）：系统服务启动时，
//! 拿本机物理网卡地址集合和「这对设备密钥生成时」记下的集合比，完全没有交集就判定磁盘被复制到了另一台机器，
//! 换成新的设备号和设备密钥。判据和并集规则见 `ddpro_ident`。
//!
//! 放在系统服务（`--service`，root / SYSTEM）里、它拉起 `--server` 之前：这里的配置是权威的那份（Linux 上用户态的
//! `--server` 启动时从它同步），记录文件也在普通用户读不到的目录。
//!
//! 记录文件两行：设备公钥（base64，标明这份地址集合属于哪对密钥）、地址集合。克隆时它和配置一起被复制过去，所以能判出来。
//! 公钥对不上（密钥被别的方式重建过）就按当前密钥重新记，不判克隆。

use ddpro::ident;
use hbb_common::{config::Config, log, sodiumoxide::crypto::sign};
use serde_json::{json, Value};

const RECORD_FILE: &str = "dd_device_macs";

/// 在 `--service` 入口调用
pub fn run() {
    let now = ident::physical_macs();
    if now.is_empty() {
        log::warn!("dd-managed: no physical network adapter found, clone check skipped");
        return;
    }
    // 从配置字段读，不用 get_key_pair / get_existing_key_pair：它们会把密钥缓存进本进程（前者没有时还会生成一对）
    let Ok(mut cfg) = serde_json::to_value(Config::get()) else {
        return;
    };
    let Some(pk) = key_pk(&cfg) else {
        return;
    };
    let pk = crate::encode64(pk);
    let path = Config::path(RECORD_FILE);
    let record = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines = record.lines();
    let rec_pk = lines.next().unwrap_or_default().trim();
    let rec_macs = ident::from_line(lines.next().unwrap_or_default());
    if rec_pk != pk || rec_macs.is_empty() {
        write_record(&pk, &now);
        return;
    }
    if ident::intersects(&now, &rec_macs) {
        let merged = ident::union(&rec_macs, &now);
        if merged != rec_macs {
            write_record(&pk, &merged);
        }
        return;
    }
    // 新密钥对和设备号一起写进去，不留空：缺了任一项配置会被当成「空配置」，Linux 上用户态同步时会跳过它、把旧身份推回来
    let (new_pk, new_sk) = sign::gen_keypair();
    let new_id = auto_id(&now);
    log::warn!(
        "dd-managed: physical network adapters {} share nothing with {} recorded for this device key, \
         treating this disk as a clone: new device ID {new_id} and key",
        ident::to_line(&now),
        ident::to_line(&rec_macs)
    );
    if let Some(m) = cfg.as_object_mut() {
        m.insert("id".to_owned(), json!(new_id));
        m.insert("key_pair".to_owned(), json!([new_sk.0.to_vec(), new_pk.0.to_vec()]));
        for k in ["enc_id", "key_confirmed", "keys_confirmed"] {
            m.remove(k);
        }
    }
    match serde_json::from_value::<Config>(cfg) {
        Ok(c) => {
            Config::set(c);
            write_record(&crate::encode64(new_pk.0), &now);
        }
        Err(e) => log::error!("dd-managed: failed to reset device identity: {e}"),
    }
}

fn key_pk(cfg: &Value) -> Option<Vec<u8>> {
    let pk: Vec<u8> = serde_json::from_value(cfg.get("key_pair")?.get(1)?.clone()).ok()?;
    (!pk.is_empty()).then_some(pk)
}

/// 和 RustDesk 自动生成设备号同一算法（网卡地址后 4 字节），用新机器的第一块物理网卡；撞号时 RustDesk 会自己换随机号
fn auto_id(macs: &ident::MacSet) -> String {
    let mac: Vec<u8> = macs
        .iter()
        .next()
        .map(|m| m.split(':').filter_map(|x| u8::from_str_radix(x, 16).ok()).collect())
        .unwrap_or_default();
    let id = mac.iter().skip(2).fold(0u32, |id, b| (id << 8) | *b as u32) & 0x1FFF_FFFF;
    id.to_string()
}

fn write_record(pk: &str, macs: &ident::MacSet) {
    let path = Config::path(RECORD_FILE);
    if let Err(e) = std::fs::write(&path, format!("{pk}\n{}\n", ident::to_line(macs))) {
        log::error!("dd-managed: write {}: {e}", path.display());
    }
}
