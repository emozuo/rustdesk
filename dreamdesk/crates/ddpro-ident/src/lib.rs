//! 设备身份的本机自查：物理网卡地址集合。生成设备密钥时记下这个集合，之后启动时和当前集合比，
//! **完全没有交集**才判定为克隆（磁盘被复制到了另一台机器），由调用方重新生成设备号和密钥。
//!
//! - 只取物理网卡：虚拟网卡（容器、网桥、虚拟机交换机、VPN）的地址会随重启变，或在克隆时原样复制过去，不能当身份。
//!   判据用各平台的标准做法：Linux 看 `/sys/class/net/<网卡>/device`（有物理设备才有），
//!   Windows 看系统网卡表的「有物理接口」标志（`ConnectorPresent`），并取出厂固化的地址（`PermanentPhysicalAddress`）。
//! - 记录取并集（[`union`]）：某次开机只认出一部分网卡、或换掉一块网卡，都不会让记录收窄、日后误判；
//!   克隆机的当前集合和复制过去的记录永远不相交，不受影响。
//! - 取不到任何网卡时是「判不了」，不是「不相交」，调用方应跳过。

use std::collections::BTreeSet;

/// 网卡地址集合：小写、冒号分隔，有序
pub type MacSet = BTreeSet<String>;

/// 本机物理网卡的地址集合（取不到时为空）
pub fn physical_macs() -> MacSet {
    platform::physical_macs()
        .into_iter()
        .filter(|m| m.len() == 6 && m.iter().any(|b| *b != 0) && m[0] & 1 == 0)
        .map(|m| m.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":"))
        .collect()
}

pub fn intersects(a: &MacSet, b: &MacSet) -> bool {
    !a.is_disjoint(b)
}

pub fn union(a: &MacSet, b: &MacSet) -> MacSet {
    a.union(b).cloned().collect()
}

/// 逗号分隔的写法（存进本机记录文件）
pub fn to_line(s: &MacSet) -> String {
    s.iter().cloned().collect::<Vec<_>>().join(",")
}

pub fn from_line(line: &str) -> MacSet {
    line.split(',')
        .map(|m| m.trim().to_ascii_lowercase())
        .filter(|m| !m.is_empty())
        .collect()
}

#[cfg(target_os = "linux")]
mod platform {
    pub fn physical_macs() -> Vec<Vec<u8>> {
        let Ok(dir) = std::fs::read_dir("/sys/class/net") else {
            return Vec::new();
        };
        dir.flatten()
            .filter(|e| e.path().join("device").exists())
            .filter_map(|e| std::fs::read_to_string(e.path().join("address")).ok())
            .filter_map(|a| {
                a.trim()
                    .split(':')
                    .map(|x| u8::from_str_radix(x, 16).ok())
                    .collect::<Option<Vec<u8>>>()
            })
            .collect()
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};

    /// `InterfaceAndOperStatusFlags` 的位：HardwareInterface、FilterInterface、ConnectorPresent ……（MIB_IF_ROW2 定义的顺序）
    const FILTER_INTERFACE: u8 = 1 << 1;
    const CONNECTOR_PRESENT: u8 = 1 << 2;

    pub fn physical_macs() -> Vec<Vec<u8>> {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        // SAFETY: GetIfTable2 成功时分配表并写入指针，用完由 FreeMibTable 释放；行数以表头 NumEntries 为准
        unsafe {
            if GetIfTable2(&mut table).is_err() || table.is_null() {
                return Vec::new();
            }
            let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
            let out = rows
                .iter()
                .filter(|r| {
                    let f = r.InterfaceAndOperStatusFlags._bitfield;
                    f & CONNECTOR_PRESENT != 0 && f & FILTER_INTERFACE == 0
                })
                .filter_map(|r| {
                    let n = (r.PhysicalAddressLength as usize).min(r.PhysicalAddress.len());
                    let permanent = &r.PermanentPhysicalAddress[..n];
                    let addr = if permanent.iter().any(|b| *b != 0) { permanent } else { &r.PhysicalAddress[..n] };
                    (n == 6).then(|| addr.to_vec())
                })
                .collect();
            FreeMibTable(table as *const _);
            out
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    pub fn physical_macs() -> Vec<Vec<u8>> {
        Vec::new()
    }
}
