//! 受管模式的应急连接补传（dreamdesk-gateway 仓库 docs/协议-连接票与设备签名.md 第七节）：应急票多半在服务端停掉时用，
//! 那时连接审计发不出去。被控端接受应急票时在本机记一条「待补传」（一行一条 JSON，重启不丢），后台每 60 秒向
//! `/api/dd/emergency-audit` 试传一轮，服务端回 `stored`（新记下或以前记过）才删。

use hbb_common::{config::Config, log};
use serde_json::Value;
use std::{
    io::Write,
    sync::{Mutex, Once},
    time::Duration,
};

const PENDING_FILE: &str = "dd_emergency_audit";
const ROUND: Duration = Duration::from_secs(60);

static FILE_LOCK: Mutex<()> = Mutex::new(());
static START: Once = Once::new();

/// 接受应急票时调用：记一条待补传，并确保后台补传在跑
pub fn record(v: Value) {
    {
        let _g = FILE_LOCK.lock().unwrap();
        let path = Config::path(PENDING_FILE);
        let res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| writeln!(f, "{v}").and_then(|_| f.sync_all()));
        if let Err(e) = res {
            log::error!("dd-managed: emergency audit not saved to {}: {e}", path.display());
        }
    }
    start();
}

/// 被控端服务启动时调用：上次没传完的接着传
pub fn start() {
    START.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("dd-emergency-audit".into())
            .spawn(|| loop {
                flush();
                std::thread::sleep(ROUND);
            });
        if let Err(e) = spawned {
            log::error!("dd-managed: emergency audit uploader not started: {e}");
        }
    });
}

fn flush() {
    let pending = {
        let _g = FILE_LOCK.lock().unwrap();
        std::fs::read_to_string(Config::path(PENDING_FILE)).unwrap_or_default()
    };
    if pending.trim().is_empty() {
        return;
    }
    let api = crate::common::get_api_server(
        Config::get_option("api-server"),
        Config::get_option("custom-rendezvous-server"),
    );
    if api.is_empty() {
        return;
    }
    let url = format!("{api}/api/dd/emergency-audit");
    // 传请求时不拿文件锁（一条可能要等十几秒），传完再按内容删掉已送达的
    let delivered: Vec<&str> = pending
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| match crate::post_request_sync(url.clone(), (*l).to_owned(), "") {
            Ok(text) => {
                let ok = serde_json::from_str::<Value>(&text).is_ok_and(|r| r.get("stored").is_some());
                if ok {
                    log::info!("dd-managed: emergency audit delivered: {l}");
                } else {
                    hbb_common::throttled_log!(Duration::from_secs(600), warn, "dd-managed: emergency audit not accepted yet: {text}");
                }
                ok
            }
            Err(e) => {
                hbb_common::throttled_log!(Duration::from_secs(600), warn, "dd-managed: emergency audit upload failed: {e}");
                false
            }
        })
        .collect();
    if delivered.is_empty() {
        return;
    }
    let _g = FILE_LOCK.lock().unwrap();
    let path = Config::path(PENDING_FILE);
    let now = std::fs::read_to_string(&path).unwrap_or_default();
    let keep: String = now
        .lines()
        .filter(|l| !l.trim().is_empty() && !delivered.contains(l))
        .map(|l| format!("{l}\n"))
        .collect();
    if let Err(e) = std::fs::write(&path, keep) {
        log::error!("dd-managed: rewrite {}: {e}", path.display());
    }
}
