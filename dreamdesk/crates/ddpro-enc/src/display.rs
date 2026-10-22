//! 被控端某块屏此刻是不是 HDR、普通内容的白有多亮（P1 第 4 项）。
//!
//! HDR 开着时 Windows 把桌面合成成线性 scRGB（FP16，1.0 = 80 尼特），普通内容的白落在用户设的
//! 「SDR 内容亮度」上，不在 1.0。主控端要靠这个亮度把普通内容还原成白、只压比它亮的高光。
//! RustDesk 的采集模块（scrap `dxgi/hdr.rs`）有同样的查询，是私有的：这里另写一份，不改它的接口。

use windows::{
    core::Interface,
    Win32::{
        Devices::Display::{
            DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig, DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
            DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
            DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
        },
        Graphics::Dxgi::{
            Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6,
        },
    },
};

/// 没读到「SDR 内容亮度」时按 Windows 的默认（80 尼特，scRGB 的 1.0）
const DEFAULT_SDR_WHITE_NITS: u32 = 80;

/// GDI 名为 `name`（如 `\\.\DISPLAY1`）的屏开着 HDR 时返回普通内容的白有多少尼特，没开（或查不到这块屏）返回 None
pub fn hdr_sdr_white_nits(name: &str) -> Option<u32> {
    is_hdr(name).then(|| sdr_white_nits(name).unwrap_or(DEFAULT_SDR_WHITE_NITS))
}

fn is_hdr(name: &str) -> bool {
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return false;
        };
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            a += 1;
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                o += 1;
                let Ok(desc) = output.GetDesc() else { continue };
                if !wide_eq(&desc.DeviceName, name) {
                    continue;
                }
                return output
                    .cast::<IDXGIOutput6>()
                    .and_then(|o6| o6.GetDesc1())
                    .is_ok_and(|d| d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020);
            }
        }
        false
    }
}

fn sdr_white_nits(name: &str) -> Option<u32> {
    unsafe {
        let (mut n_paths, mut n_modes) = (0u32, 0u32);
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes).ok().ok()?;
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
        QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, paths.as_mut_ptr(), &mut n_modes, modes.as_mut_ptr(), None)
            .ok()
            .ok()?;
        for path in &paths[..n_paths as usize] {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 || !wide_eq(&source.viewGdiDeviceName, name) {
                continue;
            }
            let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
            white.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL;
            white.header.size = std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32;
            white.header.adapterId = path.targetInfo.adapterId;
            white.header.id = path.targetInfo.id;
            // 单位：1000 = 80 尼特
            if DisplayConfigGetDeviceInfo(&mut white.header) == 0 && white.SDRWhiteLevel != 0 {
                return Some(white.SDRWhiteLevel * 80 / 1000);
            }
        }
        None
    }
}

fn wide_eq(a: &[u16], b: &str) -> bool {
    let end = a.iter().position(|&c| c == 0).unwrap_or(a.len());
    String::from_utf16_lossy(&a[..end]) == b
}
