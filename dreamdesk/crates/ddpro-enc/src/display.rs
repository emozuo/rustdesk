//! 被控端某块屏此刻是不是 HDR、普通内容的白有多亮（P1 第 4 项）。
//!
//! HDR 开着时 Windows 把桌面合成成线性 scRGB（FP16，1.0 = 80 尼特），普通内容的白落在用户设的
//! 「SDR 内容亮度」上，不在 1.0。主控端要靠这个亮度把普通内容还原成白、只压比它亮的高光。
//! RustDesk 的采集模块（scrap `dxgi/hdr.rs`）有同样的查询，是私有的：这里另写一份，不改它的接口。
//!
//! 悬浮球「HDR」（v11）：[`hdr_supported`] / [`set_hdr`] 查和切这块屏的系统 HDR（Windows「使用 HDR」那个开关）。

use windows::{
    core::Interface,
    Win32::{
        Devices::Display::{
            DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
            DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_DEVICE_INFO_TYPE, DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
            DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO,
            DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE,
            DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
        },
        Foundation::LUID,
        Graphics::Dxgi::{
            Common::{DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED},
            CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6,
        },
    },
};

/// 没读到「SDR 内容亮度」时按 Windows 的默认（80 尼特，scRGB 的 1.0）
const DEFAULT_SDR_WHITE_NITS: u32 = 80;

/// GDI 名为 `name`（如 `\\.\DISPLAY1`）的屏开着 HDR 时返回普通内容的白有多少尼特，没开（或查不到这块屏）返回 None。
/// 旋转了的屏也返回 None：RustDesk 的采集模块对它照旧压成普通 BGRA（旋转那一步只收 BGRA），只能按普通画面编
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
                if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED {
                    return false;
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
    let (adapter, id) = target(name)?;
    unsafe {
        let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
        white.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL;
        white.header.size = std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32;
        white.header.adapterId = adapter;
        white.header.id = id;
        // 单位：1000 = 80 尼特
        (DisplayConfigGetDeviceInfo(&mut white.header) == 0 && white.SDRWhiteLevel != 0).then(|| white.SDRWhiteLevel * 80 / 1000)
    }
}

/// GDI 名为 `name` 的屏接的显示器型号名（显示器自己报的名字，如「VX2723」），主控端写在屏幕标签上。
/// 查不到（没接真显示器、驱动没报）返回 None
pub fn friendly_name(name: &str) -> Option<String> {
    let (adapter, id) = target(name)?;
    unsafe {
        let mut t = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
        t.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
        t.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
        t.header.adapterId = adapter;
        t.header.id = id;
        if DisplayConfigGetDeviceInfo(&mut t.header) != 0 {
            return None;
        }
        let n = &t.monitorFriendlyDeviceName;
        let end = n.iter().position(|&c| c == 0).unwrap_or(n.len());
        let s = String::from_utf16_lossy(&n[..end]).trim().to_owned();
        (!s.is_empty()).then_some(s)
    }
}

/// GDI 名为 `name` 的屏在系统显示配置里对应的输出端（显卡, 编号）：查 / 改它的颜色设置用
fn target(name: &str) -> Option<(LUID, u32)> {
    unsafe {
        let (mut n_paths, mut n_modes) = (0u32, 0u32);
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes).ok().ok()?;
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
        QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, paths.as_mut_ptr(), &mut n_modes, modes.as_mut_ptr(), None)
            .ok()
            .ok()?;
        paths[..n_paths as usize].iter().find_map(|path| {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            (DisplayConfigGetDeviceInfo(&mut source.header) == 0 && wide_eq(&source.viewGdiDeviceName, name))
                .then_some((path.targetInfo.adapterId, path.targetInfo.id))
        })
    }
}

/// Windows 11 24H2 起的读 / 设 HDR（wingdi.h 的 DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO_2 /
/// DISPLAYCONFIG_DEVICE_INFO_SET_HDR_STATE；windows 库 0.61 还没有，照 SDK 头文件自己声明）。24H2 起旧接口的「高级颜色」
/// 在普通屏上指「自动颜色管理」：显示器模拟器也报支持，切的也不是 HDR，所以先用新接口，系统不认（24H2 之前）才退回旧接口
const GET_ADVANCED_COLOR_INFO_2: DISPLAYCONFIG_DEVICE_INFO_TYPE = DISPLAYCONFIG_DEVICE_INFO_TYPE(15);
const SET_HDR_STATE: DISPLAYCONFIG_DEVICE_INFO_TYPE = DISPLAYCONFIG_DEVICE_INFO_TYPE(16);

#[repr(C)]
#[derive(Default)]
struct AdvancedColorInfo2 {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    /// 位 3 被策略限制、位 4 支持 HDR、位 5 用户开了 HDR
    value: u32,
    color_encoding: i32,
    bits_per_color_channel: u32,
    /// 0 普通、1 广色域、2 HDR
    active_color_mode: i32,
}

#[repr(C)]
#[derive(Default)]
struct SetHdrState {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    /// 位 0 开 HDR
    value: u32,
}

/// 读这块屏的 HDR 状态用的是哪个接口
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Api {
    /// 24H2 起的新接口
    Hdr,
    /// 旧的「高级颜色」接口
    AdvancedColor,
}

/// 这块屏的 HDR 状态：(支持 HDR, HDR 开着, 用的接口, 原始状态位)。查不到这块屏时 None
fn hdr_state(adapter: LUID, id: u32) -> Option<(bool, bool, Api, u32)> {
    unsafe {
        let mut info = AdvancedColorInfo2::default();
        info.header.r#type = GET_ADVANCED_COLOR_INFO_2;
        info.header.size = std::mem::size_of::<AdvancedColorInfo2>() as u32;
        info.header.adapterId = adapter;
        info.header.id = id;
        if DisplayConfigGetDeviceInfo(&mut info.header) == 0 {
            let v = info.value;
            return Some((v & 0x10 != 0 && v & 0x8 == 0, v & 0x20 != 0, Api::Hdr, v));
        }
    }
    advanced_color(adapter, id).map(|(supported, on, v)| (supported, on, Api::AdvancedColor, v))
}

/// 旧接口读的高级颜色状态：(支持, 开着, 原始状态位)。查不到这块屏时 None
fn advanced_color(adapter: LUID, id: u32) -> Option<(bool, bool, u32)> {
    unsafe {
        let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
        info.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
        info.header.size = std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
        info.header.adapterId = adapter;
        info.header.id = id;
        if DisplayConfigGetDeviceInfo(&mut info.header) != 0 {
            return None;
        }
        // 位 0 支持、位 1 开着、位 3 被强制关掉（比如系统策略）
        let v = info.Anonymous.value;
        Some((v & 1 != 0 && v & 8 == 0, v & 2 != 0, v))
    }
}

/// GDI 名为 `name` 的屏能不能开 HDR（显示器支持、没被强制关掉），和一段说明（用的哪个接口、原始状态位；写日志用）
pub fn hdr_supported(name: &str) -> (bool, String) {
    match target(name).and_then(|(adapter, id)| hdr_state(adapter, id)) {
        Some((supported, on, api, v)) => (supported, format!("{api:?} interface, state 0x{v:x}, HDR on: {on}")),
        None => (false, "cannot read the color settings".to_owned()),
    }
}

/// 打开 / 关掉 GDI 名为 `name` 的屏的系统 HDR（Windows 设置里「使用 HDR」那个开关），返回改之前开没开。
/// 本来就是要的状态时什么也不做。系统切显示模式要一两百毫秒到一秒，返回时已经切完；屏上的采集随之重建。
/// 23H2 上用旧接口验过；24H2 起用新接口（[`hdr_state`]），新接口的切换还没在真 HDR 屏上验过
pub fn set_hdr(name: &str, on: bool) -> Result<bool, String> {
    let (adapter, id) = target(name).ok_or_else(|| format!("{name} is not an active display"))?;
    let (supported, was, api, _) = hdr_state(adapter, id).ok_or_else(|| format!("cannot read the color settings of {name}"))?;
    if !supported {
        return Err(format!("{name} does not support HDR"));
    }
    if was == on {
        return Ok(was);
    }
    let result = unsafe {
        match api {
            Api::Hdr => {
                let mut state = SetHdrState::default();
                state.header.r#type = SET_HDR_STATE;
                state.header.size = std::mem::size_of::<SetHdrState>() as u32;
                state.header.adapterId = adapter;
                state.header.id = id;
                state.value = on as u32;
                DisplayConfigSetDeviceInfo(&state.header)
            }
            Api::AdvancedColor => {
                let mut state = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE::default();
                state.header.r#type = DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE;
                state.header.size = std::mem::size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32;
                state.header.adapterId = adapter;
                state.header.id = id;
                state.Anonymous.value = on as u32;
                DisplayConfigSetDeviceInfo(&state.header)
            }
        }
    };
    match result {
        0 => Ok(was),
        e => Err(format!("cannot turn HDR {} on {name} (error {e}, {api:?} interface)", if on { "on" } else { "off" })),
    }
}

fn wide_eq(a: &[u16], b: &str) -> bool {
    let end = a.iter().position(|&c| c == 0).unwrap_or(a.len());
    String::from_utf16_lossy(&a[..end]) == b
}
