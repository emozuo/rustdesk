//! 列出本机每块屏的 GDI 名和接的显示器型号名（主控端屏幕标签上写的那个），以及查一次要多久。
//! 用法：cargo run -p ddpro-enc --example display_names

#[cfg(windows)]
fn main() {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    let mut names = Vec::new();
    unsafe {
        let factory = CreateDXGIFactory1::<IDXGIFactory1>().expect("DXGI factory");
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            a += 1;
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                o += 1;
                let d = output.GetDesc().expect("output desc");
                let end = d.DeviceName.iter().position(|&c| c == 0).unwrap_or(d.DeviceName.len());
                names.push(String::from_utf16_lossy(&d.DeviceName[..end]));
            }
        }
    }
    for name in names {
        let t = std::time::Instant::now();
        let label = ddpro_enc::friendly_name(&name);
        println!("{name}: {label:?} ({} us)", t.elapsed().as_micros());
    }
}

#[cfg(not(windows))]
fn main() {
    println!("Windows only");
}
