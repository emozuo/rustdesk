//! 被控端 Windows：调 usbip-win2 的命令行（`usbip.exe`）。驱动在内核里用 WSK 去连我们给的 `127.0.0.1` 临时端口
//! （实验 E2），握手和之后的 URB 都经那个连接转进会话连接。
//!
//! 自动重连：驱动在设备意外断开后 30 秒起会在后台反复重连（默认 20 次、约 2 小时，`usbip2_ude\Parameters\ReattachMaxAttempts`）。
//! 我们每次挂载用新的临时端口，每断一次就多一串连不上的重连任务；2026-10-22 在 209.32 上反复挂载、断开、强杀之后，驱动卡死
//! （`usbip.exe` 找不到虚拟控制器、重启设备挂住，只能断电重启）。原项目总是连同一个地址，碰巧没有这个问题。所以重连次数必须是 0
//! （[`ensure_no_reattach`]，驱动加载时读），结束时再对这个位置显式停一次（0.9.8.0 的「停」有已知缺陷、调用会直接失败，0.9.8.1 修了）。
//! 每条命令都有超时：驱动出问题时调用方不能跟着挂死。

use std::{
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// 一条 usbip.exe 命令最多等多久（挂载要经主控端握手一个来回，内网几毫秒；原项目也是 10 秒级）
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const PARAMETERS: &str = r"HKLM\SYSTEM\CurrentControlSet\Services\usbip2_ude\Parameters";

/// usbip.exe 的位置（usbip-win2 装在 Program Files\USBip）；没装返回 None
pub fn exe() -> Option<PathBuf> {
    ["ProgramW6432", "ProgramFiles"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|d| PathBuf::from(d).join("USBip").join("usbip.exe"))
        .find(|p| p.exists())
}

fn run(args: &[&str]) -> Result<Output, String> {
    let exe = exe().ok_or("the USB/IP driver (usbip-win2) is not installed")?;
    run_with_timeout(Command::new(exe).args(args), "usbip.exe")
}

fn run_with_timeout(cmd: &mut Command, what: &str) -> Result<Output, String> {
    run_with_timeout_of(cmd, what, COMMAND_TIMEOUT)
}

/// 跑一条命令，超时就结束它、报错（输出量很小，等它退出后再一次读完不会把管道写满）
fn run_with_timeout_of(cmd: &mut Command, what: &str, timeout: Duration) -> Result<Output, String> {
    let mut child = cmd
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {what}: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|e| format!("{what}: {e}")),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                child.kill().ok();
                child.wait().ok();
                return Err(format!("{what} did not finish within {} s", timeout.as_secs()));
            }
            Err(e) => return Err(format!("{what}: {e}")),
        }
    }
}

/// usbip-win2 安装程序（Inno Setup）在「应用和功能」里的登记
const UNINSTALL_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{199505b0-b93d-4521-a8c7-897818e0205a}_is1";
/// 安装程序可能要装 VC++ 运行库，比单条命令慢得多
const INSTALLER_TIMEOUT: Duration = Duration::from_secs(300);

/// 随包的安装程序：`dir` 里唯一一个 `USBip-<版本>-x64.exe`（打包脚本放进去，版本从文件名来，不在两处各写一遍）
fn bundled_installer(dir: &std::path::Path) -> Result<(PathBuf, String), String> {
    let found: Vec<(PathBuf, String)> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ver = name.strip_prefix("USBip-")?.strip_suffix("-x64.exe")?.to_owned();
            Some((e.path(), ver))
        })
        .collect();
    // 程序内升级是合并复制、不删旧文件，目录里可能还留着上一版的安装程序：取版本最高的（只认这一个的话驱动永远升不上去）
    found
        .into_iter()
        .max_by_key(|(_, ver)| ver.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>())
        .ok_or_else(|| format!("no USBip-<version>-x64.exe in {}", dir.display()))
}

fn reg_value(key: &str, name: &str) -> Option<String> {
    let o = run_with_timeout(Command::new("reg").args(["query", key, "/v", name]), "reg query").ok()?;
    let out = String::from_utf8_lossy(&o.stdout);
    let line = out.lines().find(|l| l.trim_start().starts_with(name))?;
    // 「名称  类型  值」，值里可能有空格
    let mut parts = line.split_whitespace();
    parts.next()?;
    parts.next()?;
    Some(parts.collect::<Vec<_>>().join(" "))
}

/// 这份 usbip-win2 归产品管：在「应用和功能」里被隐藏了。只有我们自己装的才隐藏，别人原先装的不隐藏（用户 2026-10-23 定：
/// 只卸自己装的），所以隐藏标记就是归属标记，早先由我们装好的机器不用迁移
fn owned() -> bool {
    reg_value(UNINSTALL_KEY, "SystemComponent").as_deref() == Some("0x1")
}

/// 装 / 升级随包的 usbip-win2（产品安装、自动升级时调）：已装的版本和随包的不同才装（重装会重载驱动，正挂着的设备会被拔一下）。
/// 照原项目实测的做法：只装驱动和命令行（main,client），不要图形界面、快捷方式；我们装的从「应用和功能」里隐藏（归产品管，产品卸载时
/// 一起卸），机器上原先就有的照样升级到随包版本（透传按这个版本测的），但不隐藏、产品卸载时也不卸；版本认卸载登记、不认 usbip.exe
/// （卸载时正被调用的 usbip.exe 删不掉会留下来，驱动却没了）。最后关掉驱动的自动重连。返回做了什么（写日志用）
pub fn install(bundle_dir: &std::path::Path) -> Result<String, String> {
    let (setup, want) = bundled_installer(bundle_dir)?;
    // 先关自动重连：驱动在安装过程中加载时读这个值（登记还在的话；装完再设一次兜底）
    let reattach_before = ensure_no_reattach();
    let have = if exe().is_some() { reg_value(UNINSTALL_KEY, "DisplayVersion") } else { None };
    // 升级前看：安装程序重新登记时可能把隐藏标记冲掉
    let ours = have.is_none() || owned();
    let mut done = Vec::new();
    if have.as_deref() != Some(want.as_str()) {
        let o = run_with_timeout_of(
            Command::new(&setup).args([
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/COMPONENTS=main,client",
                "/TASKS=vcredist",
                "/NOICONS",
            ]),
            "the USB/IP driver installer",
            INSTALLER_TIMEOUT,
        )?;
        if !o.status.success() {
            return Err(format!("the USB/IP driver installer failed ({})", o.status));
        }
        if exe().is_none() {
            return Err("the USB/IP driver installer finished but usbip.exe is missing".to_owned());
        }
        done.push(format!("installed usbip-win2 {want} (was {})", have.as_deref().unwrap_or("none")));
    } else {
        done.push(format!("usbip-win2 {want} already installed"));
    }
    if ours {
        run_with_timeout(
            Command::new("reg").args(["add", UNINSTALL_KEY, "/v", "SystemComponent", "/t", "REG_DWORD", "/d", "1", "/f"]),
            "reg add",
        )?;
    } else {
        done.push("it was installed before this product, so it stays in Apps and is kept when the product is uninstalled".to_owned());
    }
    match ensure_no_reattach() {
        Ok(true) if reattach_before.is_ok() => {
            done.push("automatic reattach was reset by the installer; turned it off again (effective after a restart)".to_owned())
        }
        Ok(_) => {}
        Err(e) => return Err(format!("cannot turn off the driver's automatic reattach: {e}")),
    }
    Ok(done.join("; "))
}

/// 卸掉产品装的 usbip-win2（产品卸载时调；它自己的卸载程序连驱动一起清）。别人原先装的不卸。返回做了什么（写日志用）
pub fn uninstall() -> Result<String, String> {
    let Some(dir) = exe().and_then(|e| e.parent().map(|p| p.to_owned())) else {
        return Ok("usbip-win2 is not installed".to_owned());
    };
    let uninstaller = dir.join("unins000.exe");
    if !uninstaller.exists() {
        return Ok("usbip-win2 has no uninstaller; left as is".to_owned());
    }
    if !owned() {
        return Ok("usbip-win2 was installed before this product; kept".to_owned());
    }
    let o = run_with_timeout_of(
        Command::new(uninstaller).args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]),
        "the USB/IP driver uninstaller",
        INSTALLER_TIMEOUT,
    )?;
    if o.status.success() {
        Ok("usbip-win2 uninstalled".to_owned())
    } else {
        Err(format!("the USB/IP driver uninstaller failed ({})", o.status))
    }
}

/// 驱动的自动重连次数是不是 0；不是就改成 0，返回 `Ok(true)` 表示刚改（驱动下次加载——一般是重启——才生效）
pub fn ensure_no_reattach() -> Result<bool, String> {
    let query = run_with_timeout(Command::new("reg").args(["query", PARAMETERS, "/v", "ReattachMaxAttempts"]), "reg query")?;
    let current = String::from_utf8_lossy(&query.stdout);
    let zero = current
        .lines()
        .find(|l| l.contains("ReattachMaxAttempts"))
        .and_then(|l| l.split_whitespace().last())
        .is_some_and(|v| v == "0x0");
    if zero {
        return Ok(false);
    }
    let set = run_with_timeout(
        Command::new("reg").args(["add", PARAMETERS, "/v", "ReattachMaxAttempts", "/t", "REG_DWORD", "/d", "0", "/f"]),
        "reg add",
    )?;
    if set.status.success() {
        Ok(true)
    } else {
        Err(text(&set))
    }
}

fn text(o: &Output) -> String {
    let mut s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
    let err = String::from_utf8_lossy(&o.stderr);
    if !err.trim().is_empty() {
        if !s.is_empty() {
            s.push_str("; ");
        }
        s.push_str(err.trim());
    }
    s
}

/// 挂载：让驱动去连 `127.0.0.1:port` 导入 `busid`，成功返回驱动的端口号（1–255）。握手要经会话连接到主控端走一个来回，
/// 这里会一直等到它完成，在单独的线程里调
pub fn attach(port: u16, busid: &str) -> Result<u32, String> {
    let port = port.to_string();
    let o = run(&["--tcp-port", &port, "attach", "-r", "127.0.0.1", "-b", busid, "--terse", "--once"])?;
    let out = text(&o);
    match out.lines().next().and_then(|l| l.trim().parse::<u32>().ok()) {
        Some(n) if o.status.success() => Ok(n),
        _ => Err(if out.is_empty() { format!("usbip attach failed ({})", o.status) } else { out }),
    }
}

/// 拔下驱动端口上的设备
pub fn detach(port_no: u32) -> Result<(), String> {
    let o = run(&["detach", "-p", &port_no.to_string()])?;
    if o.status.success() {
        Ok(())
    } else {
        Err(text(&o))
    }
}

/// 停掉驱动对这个位置的后台重连（没有在重连也不算错）
pub fn stop_reattach(port: u16, busid: &str) {
    run(&["--tcp-port", &port.to_string(), "attach", "-r", "127.0.0.1", "-b", busid, "--stop"]).ok();
}

/// 挂在 127.0.0.1 上的驱动端口号：只可能是我们挂的（进程上次没来得及收拾），首次使用时拔掉
pub fn loopback_ports() -> Vec<u32> {
    let Ok(o) = run(&["port"]) else {
        return Vec::new();
    };
    parse_ports(&String::from_utf8_lossy(&o.stdout))
}

/// `usbip port` 的输出：「Port 01: device in use ...」下面跟「-> usbip://127.0.0.1:端口/busid」
fn parse_ports(out: &str) -> Vec<u32> {
    let mut ports = Vec::new();
    let mut current = None;
    for line in out.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Port ") {
            current = rest.split(':').next().and_then(|n| n.trim().parse::<u32>().ok());
        } else if l.contains("usbip://127.0.0.1:") {
            if let Some(n) = current.take() {
                ports.push(n);
            }
        }
    }
    ports
}

#[cfg(test)]
mod tests {
    #[test]
    fn ports() {
        let out = "Imported USB devices\n====================\nPort 01: device in use at Full Speed(12Mbps)\n         Wacom Co., Ltd : CTL-6100 [Intuos (M)] (056a:0375)\n           -> usbip://127.0.0.1:10838/1-1\n           -> remote bus/dev: 001/002\nPort 02: device in use at High Speed(480Mbps)\n           -> usbip://10.0.0.5:3240/2-1\n";
        assert_eq!(super::parse_ports(out), vec![1]);
    }
}
