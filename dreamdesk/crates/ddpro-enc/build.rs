// 编译时用 Windows SDK 的 fxc 把着色器编好，嵌进程序（编译机本来就要装 VS / Windows SDK）。
// FXC 环境变量可指定 fxc.exe 路径；否则取 Windows Kits 里版本最高的那个。

use std::path::PathBuf;

const SHADERS: &[&str] = &["rgb_to_yuv444", "tile_diff"];

fn main() {
    if std::env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }
    let fxc = find_fxc().expect("fxc.exe not found: install the Windows SDK or set FXC");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    for name in SHADERS {
        let src = format!("shaders/{name}.hlsl");
        println!("cargo:rerun-if-changed={src}");
        let status = std::process::Command::new(&fxc)
            .args(["/nologo", "/T", "cs_5_0", "/E", "main", "/O3", "/WX"])
            .arg("/Fo")
            .arg(out.join(format!("{name}.cso")))
            .arg(&src)
            .status()
            .expect("failed to run fxc");
        assert!(status.success(), "fxc failed on {src}");
    }
    println!("cargo:rerun-if-env-changed=FXC");
}

fn find_fxc() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FXC") {
        return Some(PathBuf::from(p));
    }
    let bin = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&bin)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join(r"x64\fxc.exe").exists())
        .collect();
    // 目录名是 10.0.26100.0 这样的版本号，按数字比，字符串比会把 10.0.9 排在 10.0.10 后面
    let key = |p: &PathBuf| -> Vec<u32> {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.split('.').filter_map(|s| s.parse().ok()).collect())
            .unwrap_or_default()
    };
    versions.sort_by_key(key);
    versions.pop().map(|p| p.join(r"x64\fxc.exe"))
}
