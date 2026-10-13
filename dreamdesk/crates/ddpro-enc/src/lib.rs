//! 被控端专业编码：显卡上的桌面画面 → 颜色转换 → NVENC HEVC 4:4:4。
//!
//! 只支持 Windows + NVIDIA；其它平台编成空壳，`probe` 如实报不支持。

#[cfg(windows)]
mod convert;
#[cfg(windows)]
mod nvenc;
#[cfg(windows)]
mod nvenc_sys;

#[cfg(windows)]
pub use nvenc::{probe, Config, EncodedFrame, Encoder};

/// 显卡的 HEVC 编码能力（问驱动得来，不按型号猜）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caps {
    pub yuv444_8bit: bool,
    pub yuv444_10bit: bool,
    pub max_width: u32,
    pub max_height: u32,
}

#[derive(Debug)]
pub enum Error {
    /// 这台机器做不了（没有 N 卡、驱动太旧、显卡不支持 4:4:4……），原因写在里面。
    Unsupported(String),
    /// 调用方给的参数或输入不对。
    Config(String),
    /// NVENC 调用失败。
    Nvenc {
        call: &'static str,
        status: i32,
        detail: String,
    },
    #[cfg(windows)]
    D3d(windows::core::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Config(s) => write!(f, "{s}"),
            Error::Nvenc { call, status, detail } => write!(f, "{call} failed ({status}): {detail}"),
            #[cfg(windows)]
            Error::D3d(e) => write!(f, "D3D11: {e}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(windows)]
impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Error::D3d(e)
    }
}

/// 在本机第一块 NVIDIA 显卡上探测编码能力（谈判时还没有采集器，自己找卡建设备）。
#[cfg(windows)]
pub fn probe_nvidia() -> Result<Caps, Error> {
    use windows::{
        core::Interface,
        Win32::{
            Foundation::HMODULE,
            Graphics::{
                Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
                Direct3D11::{D3D11CreateDevice, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION},
                Dxgi::{CreateDXGIFactory1, IDXGIFactory1},
            },
        },
    };
    const NVIDIA: u32 = 0x10de;
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            if adapter.GetDesc1()?.VendorId != NVIDIA {
                continue;
            }
            let mut device = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
            let device = device.ok_or_else(|| Error::Config("D3D11CreateDevice returned nothing".to_owned()))?;
            return probe(device.as_raw());
        }
        Err(Error::Unsupported("no NVIDIA GPU".to_owned()))
    }
}

#[cfg(not(windows))]
pub fn probe_nvidia() -> Result<Caps, Error> {
    Err(Error::Unsupported("pro encoding needs Windows with an NVIDIA GPU".to_owned()))
}
