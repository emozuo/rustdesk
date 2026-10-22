//! 被控端专业编码：显卡上的桌面画面 → 颜色转换 → NVENC HEVC 4:4:4。
//!
//! 只支持 Windows + NVIDIA；其它平台编成空壳，`probe` 如实报不支持。

#[cfg(windows)]
mod convert;
#[cfg(windows)]
mod cuda;
#[cfg(windows)]
mod nvenc;
#[cfg(windows)]
mod nvenc_sys;

#[cfg(windows)]
pub use nvenc::{probe, Config, EncodedFrame, Encoder};

/// 采集到的 D3D11 纹理的宽高（编码器要按画面尺寸创建）。
#[cfg(windows)]
pub fn texture_size(texture: *mut std::ffi::c_void) -> Result<(u32, u32), Error> {
    use windows::{
        core::Interface,
        Win32::Graphics::Direct3D11::{ID3D11Texture2D, D3D11_TEXTURE2D_DESC},
    };
    unsafe {
        let t = ID3D11Texture2D::from_raw_borrowed(&texture)
            .ok_or_else(|| Error::Config("null texture".to_owned()))?;
        let mut d = D3D11_TEXTURE2D_DESC::default();
        t.GetDesc(&mut d);
        Ok((d.Width, d.Height))
    }
}

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
    /// CUDA 调用失败（4:4:4 10bit 编码把画面拷给 NVENC 用）。
    Cuda {
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
            Error::Cuda { call, status, detail } => write!(f, "{call} failed ({status}): {detail}"),
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

