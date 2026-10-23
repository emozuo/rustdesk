//! 悬浮球「HDR」（v11）的主控端：切 GNOME 显示器的颜色模式（BT.2100 = HDR），和 GNOME 自带的 `gdctl --color-mode` 同一个
//! D-Bus 接口（org.gnome.Mutter.DisplayConfig）。按「临时」方式应用：不弹「保留这些显示设置吗」（那个 20 秒没人点就退回），
//! 重启后回到默认——主程序意外退出没来得及恢复时，最多挂到下次重启。只改颜色模式，布局、分辨率、缩放、RGB 范围按原样带回去。

use dbus::{
    arg::{PropMap, RefArg, Variant},
    blocking::Connection,
};
use std::{collections::HashMap, time::Duration};

const DEST: &str = "org.gnome.Mutter.DisplayConfig";
const PATH: &str = "/org/gnome/Mutter/DisplayConfig";
/// 颜色模式（gdctl 的 ColorMode）：默认 / BT.2100（HDR）
const COLOR_DEFAULT: u32 = 0;
const COLOR_BT2100: u32 = 1;
/// ApplyMonitorsConfig 的应用方式：临时
const METHOD_TEMPORARY: u32 = 1;

/// GetCurrentState 的结构（gdctl 里 `(ua((ssss)a(siiddada{sv})a{sv})a(iiduba(ssss)a{sv})a{sv})`）
type Mode = (String, i32, i32, f64, f64, Vec<f64>, PropMap);
type Monitor = ((String, String, String, String), Vec<Mode>, PropMap);
type Logical = (i32, i32, f64, u32, bool, Vec<(String, String, String, String)>, PropMap);
type State = (u32, Vec<Monitor>, Vec<Logical>, PropMap);

fn current() -> Result<State, String> {
    let conn = Connection::new_session().map_err(|e| format!("connect to the session bus: {e}"))?;
    conn.with_proxy(DEST, PATH, Duration::from_secs(5))
        .method_call(DEST, "GetCurrentState", ())
        .map_err(|e| format!("read the display configuration: {e}"))
}

fn prop_u32(p: &PropMap, key: &str) -> Option<u32> {
    p.get(key).and_then(|v| v.0.as_u64()).and_then(|v| u32::try_from(v).ok())
}

fn supports_bt2100(p: &PropMap) -> bool {
    p.get("supported-color-modes")
        .and_then(|v| v.0.as_iter())
        .is_some_and(|mut modes| modes.any(|m| m.as_u64() == Some(COLOR_BT2100 as u64)))
}

/// 用着的每块屏（逻辑显示器里的）：(连接器名, 支持 HDR, 现在的颜色模式)
fn screens(s: &State) -> Vec<(String, bool, u32)> {
    let in_use: Vec<&str> = s.2.iter().flat_map(|l| l.5.iter().map(|m| m.0.as_str())).collect();
    s.1.iter()
        .filter(|m| in_use.contains(&m.0 .0.as_str()))
        .map(|m| (m.0 .0.clone(), supports_bt2100(&m.2), prop_u32(&m.2, "color-mode").unwrap_or(COLOR_DEFAULT)))
        .collect()
}

/// 本机用着的每块屏都能开 HDR（悬浮球「HDR」能不能点的本机一半；和「每块屏都开着 HDR 才谈 HDR」对得上）
pub fn available() -> bool {
    match current() {
        Ok(s) => {
            let list = screens(&s);
            !list.is_empty() && list.iter().all(|(_, hdr, _)| *hdr)
        }
        Err(e) => {
            hbb_common::log::info!("ddpro: hdr: cannot tell whether this machine can switch HDR: {e}");
            false
        }
    }
}

/// 把本机用着的每块屏切成 HDR（`on`）或默认颜色模式；返回改之前各屏的模式（会话结束时按它 [`restore`]）
pub fn set(on: bool) -> Result<Vec<(String, u32)>, String> {
    let s = current()?;
    let before: Vec<(String, u32)> = screens(&s).into_iter().map(|(c, _, mode)| (c, mode)).collect();
    let want = if on { COLOR_BT2100 } else { COLOR_DEFAULT };
    apply(&s, &before.iter().map(|(c, _)| (c.clone(), want)).collect())?;
    Ok(before)
}

/// 按 [`set`] 返回的样子恢复
pub fn restore(modes: &[(String, u32)]) -> Result<(), String> {
    let s = current()?;
    apply(&s, &modes.iter().cloned().collect())
}

/// 按当前配置原样应用一遍，只把 `modes` 里的屏的颜色模式换掉
fn apply(s: &State, modes: &HashMap<String, u32>) -> Result<(), String> {
    let monitor = |connector: &str| s.1.iter().find(|m| m.0 .0 == connector);
    let mut logical = Vec::new();
    for l in &s.2 {
        let mut monitors = Vec::new();
        for spec in &l.5 {
            let m = monitor(&spec.0).ok_or_else(|| format!("monitor {} is in a logical monitor but not listed", spec.0))?;
            let mode = m
                .1
                .iter()
                .find(|mode| mode.6.get("is-current").and_then(|v| v.0.as_u64()).is_some_and(|v| v != 0))
                .ok_or_else(|| format!("monitor {} has no current mode", spec.0))?;
            let mut props: PropMap = HashMap::new();
            if let Some(c) = modes.get(&spec.0).copied().or_else(|| prop_u32(&m.2, "color-mode")) {
                props.insert("color-mode".into(), Variant(Box::new(c) as Box<dyn RefArg>));
            }
            if let Some(r) = prop_u32(&m.2, "rgb-range") {
                props.insert("rgb-range".into(), Variant(Box::new(r) as Box<dyn RefArg>));
            }
            monitors.push((spec.0.clone(), mode.0.clone(), props));
        }
        logical.push((l.0, l.1, l.2, l.3, l.4, monitors));
    }
    let mut props: PropMap = HashMap::new();
    if s.3.get("supports-changing-layout-mode").and_then(|v| v.0.as_u64()).is_some_and(|v| v != 0) {
        if let Some(mode) = prop_u32(&s.3, "layout-mode") {
            props.insert("layout-mode".into(), Variant(Box::new(mode) as Box<dyn RefArg>));
        }
    }
    let conn = Connection::new_session().map_err(|e| format!("connect to the session bus: {e}"))?;
    conn.with_proxy(DEST, PATH, Duration::from_secs(10))
        .method_call::<(), _, _, _>(DEST, "ApplyMonitorsConfig", (s.0, METHOD_TEMPORARY, logical, props))
        .map_err(|e| format!("apply the display configuration: {e}"))
}
