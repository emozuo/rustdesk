//! NVENC：加载显卡驱动自带的 nvEncodeAPI64.dll，打开编码会话、查询能力、编 HEVC 4:4:4。
//!
//! 编码器只在创建它的线程上用，并且和采集共用同一个 D3D11 设备：NVENC 和采集共用设备时跨线程
//! 调用容易在驱动里互锁，所以从结构上排除（`Encoder` 不是 `Send`）。

use crate::{convert::Converter, nvenc_sys::*, Caps, Error};
use std::{ffi::c_void, mem, ptr};
use windows::{
    core::{Interface, PCSTR},
    Win32::{
        Graphics::Direct3D11::ID3D11Device,
        System::LibraryLoader::{GetProcAddress, LoadLibraryA},
    },
};

const fn struct_ver(v: u32) -> u32 {
    NVENCAPI_VERSION | (v << 16) | (0x7 << 28)
}
const FN_LIST_VER: u32 = struct_ver(2);
const OPEN_SESSION_VER: u32 = struct_ver(1);
const CAPS_PARAM_VER: u32 = struct_ver(1);
const PRESET_CONFIG_VER: u32 = struct_ver(4) | (1 << 31);
const CONFIG_VER: u32 = struct_ver(8) | (1 << 31);
const INIT_PARAMS_VER: u32 = struct_ver(6) | (1 << 31);
const CREATE_BITSTREAM_VER: u32 = struct_ver(1);
const REGISTER_RESOURCE_VER: u32 = struct_ver(4);
const MAP_INPUT_VER: u32 = struct_ver(4);
const PIC_PARAMS_VER: u32 = struct_ver(6) | (1 << 31);
const LOCK_BITSTREAM_VER: u32 = struct_ver(1) | (1 << 31);
const INFINITE_GOP: u32 = 0xffff_ffff;

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> GUID {
    GUID {
        Data1: d1,
        Data2: d2,
        Data3: d3,
        Data4: d4,
    }
}
const CODEC_HEVC: GUID = guid(0x790cdc88, 0x4522, 0x4d7b, [0x94, 0x25, 0xbd, 0xa9, 0x97, 0x5f, 0x76, 0x03]);
const PROFILE_HEVC_FREXT: GUID = guid(0x51ec32b5, 0x1b4c, 0x453c, [0x9c, 0xbd, 0xb6, 0x16, 0xbd, 0x62, 0x13, 0x41]);
const PRESETS: [GUID; 7] = [
    guid(0xfc0a8d3e, 0x45f8, 0x4cf8, [0x80, 0xc7, 0x29, 0x88, 0x71, 0x59, 0x0e, 0xbf]),
    guid(0xf581cfb8, 0x88d6, 0x4381, [0x93, 0xf0, 0xdf, 0x13, 0xf9, 0xc2, 0x7d, 0xab]),
    guid(0x36850110, 0x3a07, 0x441f, [0x94, 0xd5, 0x36, 0x70, 0x63, 0x1f, 0x91, 0xf6]),
    guid(0x90a7b826, 0xdf06, 0x4862, [0xb9, 0xd2, 0xcd, 0x6d, 0x73, 0xa0, 0x86, 0x81]),
    guid(0x21c6e6b4, 0x297a, 0x4cba, [0x99, 0x8f, 0xb6, 0xcb, 0xde, 0x72, 0xad, 0xe3]),
    guid(0x8e75c279, 0x6299, 0x4ab6, [0x83, 0x02, 0x0b, 0x21, 0x5a, 0x33, 0x5c, 0xf5]),
    guid(0x84848c12, 0x6f71, 0x4c13, [0x93, 0x1b, 0x53, 0xe2, 0x83, 0xf5, 0x79, 0x74]),
];

type GetMaxVersionFn = unsafe extern "system" fn(*mut u32) -> NVENCSTATUS;
type CreateInstanceFn = unsafe extern "system" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS;

/// 驱动的函数表。dll 加载后不卸载：驱动组件常驻，反复加载卸载没有好处。
struct Api(NV_ENCODE_API_FUNCTION_LIST);

impl Api {
    fn load() -> Result<Self, Error> {
        unsafe {
            let lib = LoadLibraryA(PCSTR(b"nvEncodeAPI64.dll\0".as_ptr())).map_err(|_| {
                Error::Unsupported("nvEncodeAPI64.dll not found (no NVIDIA driver)".to_owned())
            })?;
            let get_max = GetProcAddress(lib, PCSTR(b"NvEncodeAPIGetMaxSupportedVersion\0".as_ptr()))
                .ok_or_else(|| Error::Unsupported("NvEncodeAPIGetMaxSupportedVersion missing".to_owned()))?;
            let get_max: GetMaxVersionFn = mem::transmute(get_max);
            let mut max = 0u32;
            status(get_max(&mut max), "NvEncodeAPIGetMaxSupportedVersion", None)?;
            let need = (NVENCAPI_MAJOR_VERSION << 4) | NVENCAPI_MINOR_VERSION;
            if max < need {
                return Err(Error::Unsupported(format!(
                    "driver supports NVENC API {}.{}, need {}.{}: update the NVIDIA driver",
                    max >> 4,
                    max & 0xf,
                    NVENCAPI_MAJOR_VERSION,
                    NVENCAPI_MINOR_VERSION
                )));
            }
            let create = GetProcAddress(lib, PCSTR(b"NvEncodeAPICreateInstance\0".as_ptr()))
                .ok_or_else(|| Error::Unsupported("NvEncodeAPICreateInstance missing".to_owned()))?;
            let create: CreateInstanceFn = mem::transmute(create);
            let mut fns: NV_ENCODE_API_FUNCTION_LIST = mem::zeroed();
            fns.version = FN_LIST_VER;
            status(create(&mut fns), "NvEncodeAPICreateInstance", None)?;
            Ok(Self(fns))
        }
    }
}

/// 取函数表里的一项；驱动没给的项按「不支持」处理，不 panic。
macro_rules! nv {
    ($api:expr, $f:ident) => {
        $api.0
            .$f
            .ok_or_else(|| Error::Unsupported(concat!(stringify!($f), " missing").to_owned()))?
    };
}

fn status(s: NVENCSTATUS, call: &'static str, detail: Option<String>) -> Result<(), Error> {
    if s == _NVENCSTATUS::NV_ENC_SUCCESS {
        Ok(())
    } else {
        Err(Error::Nvenc {
            call,
            status: s as i32,
            detail: detail.unwrap_or_default(),
        })
    }
}

/// 一个打开的编码会话；Drop 时关闭。
struct Session {
    api: Api,
    enc: *mut c_void,
}

impl Session {
    fn open(device: *mut c_void) -> Result<Self, Error> {
        let api = Api::load()?;
        unsafe {
            let mut p: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = mem::zeroed();
            p.version = OPEN_SESSION_VER;
            p.deviceType = _NV_ENC_DEVICE_TYPE::NV_ENC_DEVICE_TYPE_DIRECTX;
            p.device = device;
            p.apiVersion = NVENCAPI_VERSION;
            let mut enc = ptr::null_mut();
            // 打开失败时还没有会话，拿不到驱动的错误描述；最常见的原因是编码会话数到了上限
            status(
                nv!(api, nvEncOpenEncodeSessionEx)(&mut p, &mut enc),
                "nvEncOpenEncodeSessionEx",
                Some("not an NVIDIA device, or the GPU's encode session limit is reached".to_owned()),
            )?;
            Ok(Self { api, enc })
        }
    }

    fn check(&self, s: NVENCSTATUS, call: &'static str) -> Result<(), Error> {
        if s == _NVENCSTATUS::NV_ENC_SUCCESS {
            return Ok(());
        }
        let detail = unsafe {
            self.api
                .0
                .nvEncGetLastErrorString
                .map(|f| f(self.enc))
                .filter(|p| !p.is_null())
                .map(|p| std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
        };
        status(s, call, detail)
    }

    fn cap(&self, which: _NV_ENC_CAPS::Type) -> Result<i32, Error> {
        unsafe {
            let mut p: NV_ENC_CAPS_PARAM = mem::zeroed();
            p.version = CAPS_PARAM_VER;
            p.capsToQuery = which;
            let mut v = 0;
            self.check(nv!(self.api, nvEncGetEncodeCaps)(self.enc, CODEC_HEVC, &mut p, &mut v), "nvEncGetEncodeCaps")?;
            Ok(v)
        }
    }

    fn caps(&self) -> Result<Caps, Error> {
        let yuv444 = self.cap(_NV_ENC_CAPS::NV_ENC_CAPS_SUPPORT_YUV444_ENCODE)? != 0;
        let ten_bit = self.cap(_NV_ENC_CAPS::NV_ENC_CAPS_SUPPORT_10BIT_ENCODE)? != 0;
        Ok(Caps {
            yuv444_8bit: yuv444,
            yuv444_10bit: yuv444 && ten_bit,
            max_width: self.cap(_NV_ENC_CAPS::NV_ENC_CAPS_WIDTH_MAX)?.max(0) as u32,
            max_height: self.cap(_NV_ENC_CAPS::NV_ENC_CAPS_HEIGHT_MAX)?.max(0) as u32,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(f) = self.api.0.nvEncDestroyEncoder {
            unsafe { f(self.enc) };
        }
    }
}

/// 在给定的 D3D11 设备上查询 HEVC 编码能力。
pub fn probe(device: *mut c_void) -> Result<Caps, Error> {
    Session::open(device)?.caps()
}

/// 编码参数。
#[derive(Debug, Clone)]
pub struct Config {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// NVENC 速度预设 1–7（1 最快、7 画质最好），按实测选。
    pub preset: u8,
    /// 颜色转换时加有序抖动；不加就是普通四舍五入（8bit 源用这个，见着色器开头的说明）。
    pub dither: bool,
    /// 静止无损用的方块边长：每帧和上一帧按方块比，0 = 不比。
    pub tile: u32,
}

/// 编出的一帧。
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub is_idr: bool,
    /// 编码器报告的这一帧平均量化参数（验证工具看码控行为用）。
    pub avg_qp: u32,
    /// 按方块比时：这一帧哪些方块变了（每位一块、低位在前、行优先），第一帧全变。
    pub changed: Option<Vec<u8>>,
}

/// HEVC 4:4:4 8bit 编码器。只能在创建它的线程上使用。
pub struct Encoder {
    session: Session,
    conv: Converter,
    registered: NV_ENC_REGISTERED_PTR,
    bitstream: NV_ENC_OUTPUT_PTR,
    cfg: Config,
    frame_idx: u64,
}

impl Encoder {
    /// `device` 是采集用的 D3D11 设备（RustDesk 采集模块的 `AdapterDevice::device`）。
    pub fn new(device: *mut c_void, cfg: Config) -> Result<Self, Error> {
        if !(1..=7).contains(&cfg.preset) {
            return Err(Error::Config(format!("preset must be 1-7, got {}", cfg.preset)));
        }
        let d3d = unsafe { ID3D11Device::from_raw_borrowed(&device) }
            .cloned()
            .ok_or_else(|| Error::Config("null D3D11 device".to_owned()))?;
        let session = Session::open(device)?;
        let caps = session.caps()?;
        // 不支持就大声失败，不悄悄退回 4:2:0
        if !caps.yuv444_8bit {
            return Err(Error::Unsupported("this GPU cannot encode HEVC 4:4:4".to_owned()));
        }
        if cfg.width > caps.max_width || cfg.height > caps.max_height {
            return Err(Error::Unsupported(format!(
                "{}x{} exceeds the encoder limit {}x{}",
                cfg.width, cfg.height, caps.max_width, caps.max_height
            )));
        }
        unsafe { Self::initialize(&session, &cfg)? };
        let conv = Converter::new(&d3d, cfg.width, cfg.height, cfg.dither, cfg.tile)?;

        let mut enc = Self {
            session,
            conv,
            registered: ptr::null_mut(),
            bitstream: ptr::null_mut(),
            cfg,
            frame_idx: 0,
        };
        unsafe {
            let mut r: NV_ENC_REGISTER_RESOURCE = mem::zeroed();
            r.version = REGISTER_RESOURCE_VER;
            r.resourceType = _NV_ENC_INPUT_RESOURCE_TYPE::NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX;
            r.width = enc.cfg.width;
            r.height = enc.cfg.height;
            r.resourceToRegister = enc.conv.output_ptr();
            r.bufferFormat = _NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_AYUV;
            r.bufferUsage = _NV_ENC_BUFFER_USAGE::NV_ENC_INPUT_IMAGE;
            let s = nv!(enc.session.api, nvEncRegisterResource)(enc.session.enc, &mut r);
            enc.session.check(s, "nvEncRegisterResource")?;
            enc.registered = r.registeredResource;

            let mut b: NV_ENC_CREATE_BITSTREAM_BUFFER = mem::zeroed();
            b.version = CREATE_BITSTREAM_VER;
            let s = nv!(enc.session.api, nvEncCreateBitstreamBuffer)(enc.session.enc, &mut b);
            enc.session.check(s, "nvEncCreateBitstreamBuffer")?;
            enc.bitstream = b.bitstreamBuffer;
        }
        Ok(enc)
    }

    unsafe fn initialize(session: &Session, cfg: &Config) -> Result<(), Error> {
        let preset = PRESETS[(cfg.preset - 1) as usize];
        let tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_LOW_LATENCY;
        let mut pc: NV_ENC_PRESET_CONFIG = mem::zeroed();
        pc.version = PRESET_CONFIG_VER;
        pc.presetCfg.version = CONFIG_VER;
        let s = nv!(session.api, nvEncGetEncodePresetConfigEx)(session.enc, CODEC_HEVC, preset, tuning, &mut pc);
        session.check(s, "nvEncGetEncodePresetConfigEx")?;

        let mut c = pc.presetCfg;
        c.profileGUID = PROFILE_HEVC_FREXT;
        // 关键帧只在需要时发（开会话、对端要求），不定期插；不用 B 帧（会多一帧延迟）
        c.gopLength = INFINITE_GOP;
        c.frameIntervalP = 1;
        let bitrate = cfg.bitrate_kbps.saturating_mul(1000);
        c.rcParams.rateControlMode = _NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_CBR;
        c.rcParams.averageBitRate = bitrate;
        c.rcParams.maxBitRate = bitrate;
        // 码率缓冲只给一帧的量：单帧不会大到要排队好几帧才发得完
        c.rcParams.vbvBufferSize = bitrate / cfg.fps.max(1);
        c.rcParams.vbvInitialDelay = c.rcParams.vbvBufferSize;

        let hevc = &mut c.encodeCodecConfig.hevcConfig;
        hevc.set_chromaFormatIDC(3);
        hevc.set_pixelBitDepthMinus8(0);
        hevc.set_repeatSPSPPS(1);
        hevc.idrPeriod = INFINITE_GOP;
        // 码流里写明颜色规则：BT.709、全范围，和颜色转换着色器一致
        let vui = &mut hevc.hevcVUIParameters;
        vui.videoSignalTypePresentFlag = 1;
        vui.videoFormat = _NV_ENC_VUI_VIDEO_FORMAT::NV_ENC_VUI_VIDEO_FORMAT_UNSPECIFIED;
        vui.videoFullRangeFlag = 1;
        vui.colourDescriptionPresentFlag = 1;
        vui.colourPrimaries = _NV_ENC_VUI_COLOR_PRIMARIES::NV_ENC_VUI_COLOR_PRIMARIES_BT709;
        vui.transferCharacteristics = _NV_ENC_VUI_TRANSFER_CHARACTERISTIC::NV_ENC_VUI_TRANSFER_CHARACTERISTIC_BT709;
        vui.colourMatrix = _NV_ENC_VUI_MATRIX_COEFFS::NV_ENC_VUI_MATRIX_COEFFS_BT709;

        let mut ip: NV_ENC_INITIALIZE_PARAMS = mem::zeroed();
        ip.version = INIT_PARAMS_VER;
        ip.encodeGUID = CODEC_HEVC;
        ip.presetGUID = preset;
        ip.encodeWidth = cfg.width;
        ip.encodeHeight = cfg.height;
        ip.darWidth = cfg.width;
        ip.darHeight = cfg.height;
        ip.frameRateNum = cfg.fps;
        ip.frameRateDen = 1;
        ip.enablePTD = 1;
        ip.encodeConfig = &mut c;
        ip.maxEncodeWidth = cfg.width;
        ip.maxEncodeHeight = cfg.height;
        ip.tuningInfo = tuning;
        ip.bufferFormat = _NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_AYUV;
        let s = nv!(session.api, nvEncInitializeEncoder)(session.enc, &mut ip);
        session.check(s, "nvEncInitializeEncoder")
    }

    /// 编一帧。`src` 是采集到的桌面纹理（BGRA 8bit，和编码器同一设备）；`None` 表示画面没变，
    /// 重编上一帧（静止时仍按帧率出帧）。第一帧总是关键帧。
    pub fn encode(&mut self, src: Option<*mut c_void>, force_idr: bool) -> Result<EncodedFrame, Error> {
        let changed = match src {
            Some(src) => self.conv.load_source(src)?,
            None => self.conv.unchanged(),
        };
        self.conv.convert();
        let is_idr = force_idr || self.frame_idx == 0;
        unsafe {
            let mut m: NV_ENC_MAP_INPUT_RESOURCE = mem::zeroed();
            m.version = MAP_INPUT_VER;
            m.registeredResource = self.registered;
            let s = nv!(self.session.api, nvEncMapInputResource)(self.session.enc, &mut m);
            self.session.check(s, "nvEncMapInputResource")?;

            let result = self.encode_mapped(m.mappedResource, m.mappedBufferFmt, is_idr);

            let s = nv!(self.session.api, nvEncUnmapInputResource)(self.session.enc, m.mappedResource);
            let unmapped = self.session.check(s, "nvEncUnmapInputResource");
            let mut frame = result?;
            unmapped?;
            self.frame_idx += 1;
            frame.changed = changed;
            Ok(frame)
        }
    }

    unsafe fn encode_mapped(
        &mut self,
        input: NV_ENC_INPUT_PTR,
        fmt: NV_ENC_BUFFER_FORMAT,
        is_idr: bool,
    ) -> Result<EncodedFrame, Error> {
        let mut p: NV_ENC_PIC_PARAMS = mem::zeroed();
        p.version = PIC_PARAMS_VER;
        p.inputWidth = self.cfg.width;
        p.inputHeight = self.cfg.height;
        p.inputBuffer = input;
        p.outputBitstream = self.bitstream;
        p.bufferFmt = fmt;
        p.pictureStruct = _NV_ENC_PIC_STRUCT::NV_ENC_PIC_STRUCT_FRAME;
        p.inputTimeStamp = self.frame_idx;
        if is_idr {
            p.encodePicFlags = (_NV_ENC_PIC_FLAGS::NV_ENC_PIC_FLAG_FORCEIDR | _NV_ENC_PIC_FLAGS::NV_ENC_PIC_FLAG_OUTPUT_SPSPPS) as u32;
        }
        let s = nv!(self.session.api, nvEncEncodePicture)(self.session.enc, &mut p);
        self.session.check(s, "nvEncEncodePicture")?;

        let mut l: NV_ENC_LOCK_BITSTREAM = mem::zeroed();
        l.version = LOCK_BITSTREAM_VER;
        l.outputBitstream = self.bitstream;
        let s = nv!(self.session.api, nvEncLockBitstream)(self.session.enc, &mut l);
        self.session.check(s, "nvEncLockBitstream")?;
        let data = std::slice::from_raw_parts(l.bitstreamBufferPtr as *const u8, l.bitstreamSizeInBytes as usize).to_vec();
        let avg_qp = l.frameAvgQP;
        let s = nv!(self.session.api, nvEncUnlockBitstream)(self.session.enc, self.bitstream);
        self.session.check(s, "nvEncUnlockBitstream")?;
        Ok(EncodedFrame { data, is_idr, avg_qp, changed: None })
    }

    /// 把最近一次载入的源画面读回内存（BGRA，逐行紧排）。只给颜色比对用，正常编码不走这里。
    pub fn read_source_bgra(&self) -> Result<Vec<u8>, Error> {
        self.conv.read_source_bgra()
    }

    /// 从最近一次载入的源画面读回几块原始像素（静止无损用）：`rects` 是 (x, y, 宽, 高)，返回每块的 BGRA（逐行紧排）。
    pub fn read_tiles(&mut self, rects: &[(u32, u32, u32, u32)]) -> Result<Vec<Vec<u8>>, Error> {
        self.conv.read_tiles(rects)
    }

    /// 开 / 关抖动，下一帧生效：只改颜色转换的一个参数，不重建编码器、不出关键帧。
    pub fn set_dither(&mut self, dither: bool) {
        if self.cfg.dither != dither {
            self.cfg.dither = dither;
            self.conv.set_dither(dither);
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            if !self.bitstream.is_null() {
                if let Some(f) = self.session.api.0.nvEncDestroyBitstreamBuffer {
                    f(self.session.enc, self.bitstream);
                }
            }
            if !self.registered.is_null() {
                if let Some(f) = self.session.api.0.nvEncUnregisterResource {
                    f(self.session.enc, self.registered);
                }
            }
        }
    }
}
