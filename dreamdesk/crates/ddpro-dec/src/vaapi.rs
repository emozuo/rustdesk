//! VAAPI 设备、HEVC 硬解、视频处理单元转 RGB、读回（验证用）、导出 dmabuf（交给显示用）。
//!
//! libva 的函数由 FFmpeg 在运行时加载，从设备上下文里的函数表 [`ffi::VAAPIDynLoadFunctions`] 调用，
//! 和 FFmpeg 解码用的是同一份 libva、同一个显示连接。

use crate::ffi;
use std::{
    ffi::{c_void, CStr},
    fmt,
    io::{Seek, SeekFrom},
    mem::{size_of, zeroed},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        raw::c_char,
    },
    ptr::{self, null_mut},
};

#[derive(Debug)]
pub struct Error(pub(crate) String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// FFmpeg 的 AVERROR(EAGAIN)：解码器要更多输入才能出帧
const AVERROR_EAGAIN: i32 = -11;
const VA_SUCCESS: ffi::VAStatus = ffi::VA_STATUS_SUCCESS as ffi::VAStatus;
const VA_UNSUPPORTED_PROFILE: ffi::VAStatus = ffi::VA_STATUS_ERROR_UNSUPPORTED_PROFILE as ffi::VAStatus;
const VA_INVALID: u32 = ffi::VA_INVALID_ID;

fn av_error(code: i32, what: &str) -> Error {
    let mut buf = [0 as c_char; 128];
    unsafe { ffi::av_strerror(code, buf.as_mut_ptr(), buf.len()) };
    let msg = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    Error(format!("{what}: {msg} ({code})"))
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// 调 libva 的函数，状态不是成功就报错（`$what` 写明在做什么）。
macro_rules! va_call {
    ($dev:expr, $name:ident ( $($arg:expr),* $(,)? ), $what:expr) => {{
        let va = $dev.va()?;
        let f = va.$name.ok_or_else(|| Error(format!("libva has no {}", stringify!($name))))?;
        let st = unsafe { f($($arg),*) };
        $dev.check(st, $what)
    }};
}

// ------------------------------------------------------------------ 设备

/// 显卡的 VAAPI 设备（FFmpeg 的设备上下文）。没有桌面时自动用 DRM 渲染节点。
pub struct Device {
    buf: *mut ffi::AVBufferRef,
}

// FFmpeg 的设备上下文是引用计数的只读对象；解码和转换在同一线程里串行调用
unsafe impl Send for Device {}

#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub hevc_444_8bit: bool,
    pub hevc_444_10bit: bool,
    /// 视频处理单元（颜色转换、缩放）
    pub video_proc: bool,
}

impl Device {
    pub fn open() -> Result<Self> {
        let mut buf = null_mut();
        let r = unsafe {
            ffi::av_hwdevice_ctx_create(&mut buf, ffi::AV_HWDEVICE_TYPE_VAAPI, ptr::null(), null_mut(), 0)
        };
        if r < 0 {
            return Err(av_error(r, "open the VAAPI device"));
        }
        Ok(Self { buf })
    }

    /// 同一个设备再拿一份引用（解码器、转换器各持一份）。
    pub fn share(&self) -> Result<Self> {
        let buf = unsafe { ffi::av_buffer_ref(self.buf) };
        if buf.is_null() {
            return Err(Error("out of memory sharing the VAAPI device".into()));
        }
        Ok(Self { buf })
    }

    fn hwctx(&self) -> &ffi::AVVAAPIDeviceContext {
        unsafe {
            let dev = (*self.buf).data as *const ffi::AVHWDeviceContext;
            &*((*dev).hwctx as *const ffi::AVVAAPIDeviceContext)
        }
    }

    fn display(&self) -> ffi::VADisplay {
        self.hwctx().display
    }

    fn va(&self) -> Result<&ffi::VAAPIDynLoadFunctions> {
        let funcs = self.hwctx().funcs;
        if funcs.is_null() {
            return Err(Error("FFmpeg did not load libva".into()));
        }
        Ok(unsafe { &*funcs })
    }

    fn check(&self, st: ffi::VAStatus, what: &str) -> Result<()> {
        if st == VA_SUCCESS {
            return Ok(());
        }
        let msg = self
            .va()
            .ok()
            .and_then(|va| va.vaErrorStr)
            .map(|f| cstr(unsafe { f(st) }))
            .unwrap_or_default();
        Err(Error(format!("{what}: {msg} ({st:#x})")))
    }

    /// 驱动名和版本，如「Intel iHD driver ... 25.2.4」。
    pub fn vendor(&self) -> Result<String> {
        let va = self.va()?;
        let f = va.vaQueryVendorString.ok_or_else(|| Error("libva has no vaQueryVendorString".into()))?;
        Ok(cstr(unsafe { f(self.display()) }))
    }

    pub fn probe(&self) -> Result<Caps> {
        Ok(Caps {
            hevc_444_8bit: self.has_entrypoint(ffi::VAProfileHEVCMain444, ffi::VAEntrypointVLD)?,
            hevc_444_10bit: self.has_entrypoint(ffi::VAProfileHEVCMain444_10, ffi::VAEntrypointVLD)?,
            video_proc: self.has_entrypoint(ffi::VAProfileNone, ffi::VAEntrypointVideoProc)?,
        })
    }

    fn has_entrypoint(&self, profile: ffi::VAProfile, entrypoint: ffi::VAEntrypoint) -> Result<bool> {
        let va = self.va()?;
        let max = va.vaMaxNumEntrypoints.ok_or_else(|| Error("libva has no vaMaxNumEntrypoints".into()))?;
        let query = va
            .vaQueryConfigEntrypoints
            .ok_or_else(|| Error("libva has no vaQueryConfigEntrypoints".into()))?;
        let mut list = vec![0 as ffi::VAEntrypoint; unsafe { max(self.display()) }.max(1) as usize];
        let mut count = 0;
        let st = unsafe { query(self.display(), profile, list.as_mut_ptr(), &mut count) };
        if st == VA_UNSUPPORTED_PROFILE {
            return Ok(false);
        }
        self.check(st, "query entrypoints")?;
        Ok(list[..count.max(0) as usize].contains(&entrypoint))
    }

    /// 等显卡把这张画面做完（解码或转换）。
    fn sync(&self, surface: ffi::VASurfaceID) -> Result<()> {
        va_call!(self, vaSyncSurface(self.display(), surface), "wait for the GPU")
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.buf) };
    }
}

// ------------------------------------------------------------------ 解码

/// 显存里的一帧解码结果。
pub struct Frame {
    f: *mut ffi::AVFrame,
}

unsafe impl Send for Frame {}

/// 解码器实际给出的表面格式（按真正的表面算，不按码流声称的）。
#[derive(Debug, Clone)]
pub struct Format {
    pub name: String,
    pub chroma_444: bool,
    pub bit_depth: u32,
}

impl Frame {
    fn surface(&self) -> ffi::VASurfaceID {
        unsafe { (*self.f).data[3] as usize as ffi::VASurfaceID }
    }

    pub fn width(&self) -> u32 {
        unsafe { (*self.f).width as u32 }
    }

    pub fn height(&self) -> u32 {
        unsafe { (*self.f).height as u32 }
    }

    pub fn format(&self) -> Format {
        unsafe {
            let hw = (*self.f).hw_frames_ctx;
            if hw.is_null() {
                return Format {
                    name: "unknown".into(),
                    chroma_444: false,
                    bit_depth: 0,
                };
            }
            let sw = (*((*hw).data as *const ffi::AVHWFramesContext)).sw_format;
            let name = cstr(ffi::av_get_pix_fmt_name(sw));
            let desc = ffi::av_pix_fmt_desc_get(sw);
            if desc.is_null() {
                return Format {
                    name,
                    chroma_444: false,
                    bit_depth: 0,
                };
            }
            let d = &*desc;
            Format {
                name,
                chroma_444: d.nb_components >= 3 && d.log2_chroma_w == 0 && d.log2_chroma_h == 0,
                bit_depth: d.comp[0].depth as u32,
            }
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe { ffi::av_frame_free(&mut self.f) };
    }
}

/// FFmpeg 只给出 VAAPI 表面；没有硬解就失败，不退回软解（专业模式不静默降级）。
unsafe extern "C" fn pick_vaapi(
    _ctx: *mut ffi::AVCodecContext,
    formats: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    let mut p = formats;
    while *p != ffi::AV_PIX_FMT_NONE {
        if *p == ffi::AV_PIX_FMT_VAAPI {
            return *p;
        }
        p = p.add(1);
    }
    ffi::AV_PIX_FMT_NONE
}

/// HEVC 硬解。低延迟、无 B 帧：送一帧就出一帧。
pub struct Decoder {
    dev: Device,
    ctx: *mut ffi::AVCodecContext,
    pkt: *mut ffi::AVPacket,
}

unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new(dev: &Device) -> Result<Self> {
        unsafe {
            let codec = ffi::avcodec_find_decoder(ffi::AV_CODEC_ID_HEVC);
            if codec.is_null() {
                return Err(Error("FFmpeg has no HEVC decoder".into()));
            }
            let d = Self {
                dev: dev.share()?,
                ctx: ffi::avcodec_alloc_context3(codec),
                pkt: ffi::av_packet_alloc(),
            };
            if d.ctx.is_null() || d.pkt.is_null() {
                return Err(Error("out of memory creating the HEVC decoder".into()));
            }
            (*d.ctx).hw_device_ctx = ffi::av_buffer_ref(dev.buf);
            // 引用失败（内存不够）时这里是空的，解码器就没有硬件设备可用
            if (*d.ctx).hw_device_ctx.is_null() {
                return Err(Error("out of memory creating the HEVC decoder".into()));
            }
            (*d.ctx).get_format = Some(pick_vaapi);
            (*d.ctx).flags |= ffi::AV_CODEC_FLAG_LOW_DELAY as i32;
            (*d.ctx).thread_count = 1;
            let r = ffi::avcodec_open2(d.ctx, codec, null_mut());
            if r < 0 {
                return Err(av_error(r, "open the HEVC decoder"));
            }
            Ok(d)
        }
    }

    /// 送入一个完整的访问单元（带起始码），解出就返回；还要更多输入时返回 None。
    /// 出错（如丢包后参考帧缺失）返回错误，解码器本身可继续用，等下一个关键帧。
    pub fn decode(&mut self, au: &[u8]) -> Result<Option<Frame>> {
        let len = i32::try_from(au.len()).map_err(|_| Error("access unit too large".into()))?;
        unsafe {
            let r = ffi::av_new_packet(self.pkt, len);
            if r < 0 {
                return Err(av_error(r, "allocate a packet"));
            }
            ptr::copy_nonoverlapping(au.as_ptr(), (*self.pkt).data, au.len());
            let r = ffi::avcodec_send_packet(self.ctx, self.pkt);
            ffi::av_packet_unref(self.pkt);
            if r < 0 {
                return Err(av_error(r, "decode"));
            }
            let f = ffi::av_frame_alloc();
            if f.is_null() {
                return Err(Error("out of memory allocating a frame".into()));
            }
            let frame = Frame { f };
            let r = ffi::avcodec_receive_frame(self.ctx, f);
            if r == AVERROR_EAGAIN {
                return Ok(None);
            }
            if r < 0 {
                return Err(av_error(r, "decode"));
            }
            if (*f).format != ffi::AV_PIX_FMT_VAAPI {
                return Err(Error("the decoder returned a software frame instead of a VAAPI surface".into()));
            }
            Ok(Some(frame))
        }
    }

    /// 等显卡把这一帧解完（`decode` 返回时显卡可能还在画）。
    pub fn wait(&self, frame: &Frame) -> Result<()> {
        self.dev.sync(frame.surface())
    }

    /// 把这一帧所在的显卡画面导出成 dmabuf（着色器转 RGB 时直接导入，不拷贝）。
    pub(crate) fn export(&self, frame: &Frame) -> Result<Exported> {
        export_surface(&self.dev, frame.surface(), ffi::VA_EXPORT_SURFACE_READ_ONLY, "export the decoded surface as dmabuf")
    }
}

impl Frame {
    /// 显卡画面的编号：解码器的画面池里反复用同几张，着色器按它缓存导入结果
    pub(crate) fn surface_id(&self) -> u32 {
        self.surface()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            ffi::av_packet_free(&mut self.pkt);
            ffi::avcodec_free_context(&mut self.ctx);
        }
    }
}

// ------------------------------------------------------------------ 转 RGB

/// 输出 RGB 一侧按哪个颜色标准解释。YUV 一侧固定 BT.709 全范围（被控端编码时就是这么转的）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RgbStandard {
    Bt709,
    Srgb,
}

/// 显卡视频处理单元：解码出的 YUV 4:4:4 → RGB（XRGB8888，内存里按 B G R X 排），顺带等比缩放。
/// 预先建好一组输出画面，调用方轮流用（显示端还拿着上一张时写下一张）。
pub struct Converter {
    dev: Device,
    config: ffi::VAConfigID,
    context: ffi::VAContextID,
    surfaces: Vec<ffi::VASurfaceID>,
    width: u32,
    height: u32,
}

unsafe impl Send for Converter {}

impl Converter {
    /// 自己建 `count` 张输出画面（驱动默认排布，iHD 给的是线性）。
    pub fn new(dev: &Device, width: u32, height: u32, count: usize) -> Result<Self> {
        Self::build(dev, width, height, |c| {
            let display = c.dev.display();
            let mut attr: ffi::VASurfaceAttrib = unsafe { zeroed() };
            attr.type_ = ffi::VASurfaceAttribPixelFormat;
            attr.flags = ffi::VA_SURFACE_ATTRIB_SETTABLE;
            attr.value.type_ = ffi::VAGenericValueTypeInteger;
            attr.value.value.i = ffi::VA_FOURCC_BGRX as i32;
            let mut surfaces = vec![VA_INVALID; count];
            va_call!(
                c.dev,
                vaCreateSurfaces(
                    display,
                    ffi::VA_RT_FORMAT_RGB32,
                    width,
                    height,
                    surfaces.as_mut_ptr(),
                    count as u32,
                    &mut attr,
                    1
                ),
                "create the RGB output surfaces"
            )?;
            c.surfaces = surfaces;
            Ok(())
        })
    }

    /// 输出画面用别处分配好的 dmabuf（[`crate::GlConverter::new`] 用 GBM 按分块排布分配的那组）：要缩放时视频处理单元
    /// 也画进同一组，窗口只登记一组画面。驱动建画面时不认指定的排布（iHD 照旧给线性），所以排布由 GBM 定、这里导入。
    pub fn with_images(dev: &Device, width: u32, height: u32, images: &[Exported]) -> Result<Self> {
        Self::build(dev, width, height, |c| {
            let display = c.dev.display();
            for img in images {
                let (fd, layer, plane) = img.packed_plane()?;
                // dmabuf 的大小只能从文件末尾的位置读（fstat 报 0）
                let size = fd
                    .try_clone()
                    .and_then(|f| std::fs::File::from(f).seek(SeekFrom::End(0)))
                    .map_err(|e| Error(format!("size of an output image: {e}")))?;
                let mut d: ffi::VADRMPRIMESurfaceDescriptor = unsafe { zeroed() };
                d.fourcc = ffi::VA_FOURCC_BGRX;
                d.width = width;
                d.height = height;
                d.num_objects = 1;
                d.objects[0].fd = fd.as_raw_fd();
                d.objects[0].size = size as u32;
                d.objects[0].drm_format_modifier = img.modifiers.first().copied().unwrap_or(0);
                d.num_layers = 1;
                d.layers[0].drm_format = layer.drm_format;
                d.layers[0].num_planes = 1;
                d.layers[0].object_index[0] = 0;
                d.layers[0].offset[0] = plane.offset;
                d.layers[0].pitch[0] = plane.pitch;
                let mut attrs: [ffi::VASurfaceAttrib; 2] = unsafe { zeroed() };
                attrs[0].type_ = ffi::VASurfaceAttribMemoryType;
                attrs[0].flags = ffi::VA_SURFACE_ATTRIB_SETTABLE;
                attrs[0].value.type_ = ffi::VAGenericValueTypeInteger;
                attrs[0].value.value.i = ffi::VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2 as i32;
                attrs[1].type_ = ffi::VASurfaceAttribExternalBufferDescriptor;
                attrs[1].flags = ffi::VA_SURFACE_ATTRIB_SETTABLE;
                attrs[1].value.type_ = ffi::VAGenericValueTypePointer;
                attrs[1].value.value.p = &mut d as *mut _ as *mut c_void;
                let mut s = VA_INVALID;
                va_call!(
                    c.dev,
                    vaCreateSurfaces(display, ffi::VA_RT_FORMAT_RGB32, width, height, &mut s, 1, attrs.as_mut_ptr(), 2),
                    "import an output image"
                )?;
                // 逐张记下：中途失败时析构会把已经导入的放掉
                c.surfaces.push(s);
            }
            Ok(())
        })
    }

    /// 建视频处理配置，`surfaces` 填好输出画面，再建处理上下文。
    fn build(dev: &Device, width: u32, height: u32, surfaces: impl FnOnce(&mut Self) -> Result<()>) -> Result<Self> {
        let mut c = Self {
            dev: dev.share()?,
            config: VA_INVALID,
            context: VA_INVALID,
            surfaces: Vec::new(),
            width,
            height,
        };
        let display = c.dev.display();
        va_call!(
            c.dev,
            vaCreateConfig(display, ffi::VAProfileNone, ffi::VAEntrypointVideoProc, null_mut(), 0, &mut c.config),
            "create the video processing config"
        )?;
        surfaces(&mut c)?;
        let count = c.surfaces.len();
        va_call!(
            c.dev,
            vaCreateContext(
                display,
                c.config,
                width as i32,
                height as i32,
                ffi::VA_PROGRESSIVE as i32,
                c.surfaces.as_mut_ptr(),
                count as i32,
                &mut c.context
            ),
            "create the video processing context"
        )?;
        Ok(c)
    }

    pub fn count(&self) -> usize {
        self.surfaces.len()
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 把 `src` 转成 RGB 写进第 `index` 张输出画面：等比缩放、居中、四周黑边。只提交，要等做完用 [`Converter::wait`]。
    pub fn convert(&mut self, src: &Frame, index: usize, rgb: RgbStandard) -> Result<()> {
        let dst = self.surfaces[index];
        let display = self.dev.display();
        let src_rect = ffi::VARectangle {
            x: 0,
            y: 0,
            width: src.width() as u16,
            height: src.height() as u16,
        };
        let out_rect = fit(src.width(), src.height(), self.width, self.height);
        let mut p: ffi::VAProcPipelineParameterBuffer = unsafe { zeroed() };
        p.surface = src.surface();
        p.surface_region = &src_rect;
        p.surface_color_standard = ffi::VAProcColorStandardBT709;
        p.output_region = &out_rect;
        p.output_background_color = 0xff00_0000;
        p.output_color_standard = match rgb {
            RgbStandard::Bt709 => ffi::VAProcColorStandardBT709,
            RgbStandard::Srgb => ffi::VAProcColorStandardSRGB,
        };
        p.input_color_properties.color_range = ffi::VA_SOURCE_RANGE_FULL as u8;
        p.output_color_properties.color_range = ffi::VA_SOURCE_RANGE_FULL as u8;
        if out_rect.width as u32 != src.width() || out_rect.height as u32 != src.height() {
            p.filter_flags = ffi::VA_FILTER_SCALING_HQ;
        }
        let mut buf = VA_INVALID;
        va_call!(
            self.dev,
            vaCreateBuffer(
                display,
                self.context,
                ffi::VAProcPipelineParameterBufferType,
                size_of::<ffi::VAProcPipelineParameterBuffer>() as u32,
                1,
                &mut p as *mut _ as *mut c_void,
                &mut buf
            ),
            "create the video processing parameters"
        )?;
        let r = self.render(dst, &mut buf);
        if let Ok(Some(f)) = self.dev.va().map(|va| va.vaDestroyBuffer) {
            unsafe { f(display, buf) };
        }
        r
    }

    fn render(&self, dst: ffi::VASurfaceID, buf: &mut ffi::VABufferID) -> Result<()> {
        let display = self.dev.display();
        va_call!(self.dev, vaBeginPicture(display, self.context, dst), "start the color conversion")?;
        // 提交失败也要结束这次处理，不然这个处理上下文一直停在「开始了没结束」，下一次开始会被拒绝
        let rendered = va_call!(self.dev, vaRenderPicture(display, self.context, buf, 1), "submit the color conversion");
        let ended = va_call!(self.dev, vaEndPicture(display, self.context), "finish the color conversion");
        rendered.and(ended)
    }

    /// 等显卡把第 `index` 张输出画面转完。
    pub fn wait(&self, index: usize) -> Result<()> {
        self.dev.sync(self.surfaces[index])
    }

    /// 读回第 `index` 张输出画面，按 R G B 逐像素排好（验证颜色用，正式显示不走这条路）。
    pub fn read_rgb(&self, index: usize) -> Result<Vec<u8>> {
        let display = self.dev.display();
        let mut format: ffi::VAImageFormat = unsafe { zeroed() };
        format.fourcc = ffi::VA_FOURCC_BGRX;
        format.byte_order = ffi::VA_LSB_FIRST;
        format.bits_per_pixel = 32;
        format.depth = 24;
        format.red_mask = 0x00ff_0000;
        format.green_mask = 0x0000_ff00;
        format.blue_mask = 0x0000_00ff;
        let mut image: ffi::VAImage = unsafe { zeroed() };
        va_call!(
            self.dev,
            vaCreateImage(display, &mut format, self.width as i32, self.height as i32, &mut image),
            "create an image for read-back"
        )?;
        let r = self.copy_out(self.surfaces[index], &image);
        if let Ok(Some(f)) = self.dev.va().map(|va| va.vaDestroyImage) {
            unsafe { f(display, image.image_id) };
        }
        r
    }

    fn copy_out(&self, surface: ffi::VASurfaceID, image: &ffi::VAImage) -> Result<Vec<u8>> {
        let display = self.dev.display();
        let (w, h) = (self.width as usize, self.height as usize);
        va_call!(
            self.dev,
            vaGetImage(display, surface, 0, 0, self.width, self.height, image.image_id),
            "read back the RGB surface"
        )?;
        let mut p: *mut c_void = null_mut();
        va_call!(self.dev, vaMapBuffer(display, image.buf, &mut p), "map the read-back image")?;
        let (pitch, offset) = (image.pitches[0] as usize, image.offsets[0] as usize);
        let mut out = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            let row = unsafe { std::slice::from_raw_parts((p as *const u8).add(offset + y * pitch), w * 4) };
            for px in row.chunks_exact(4) {
                out.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        va_call!(self.dev, vaUnmapBuffer(display, image.buf), "unmap the read-back image")?;
        Ok(out)
    }

    /// 把第 `index` 张输出画面导出成 dmabuf（交给显示用，零拷贝）。
    pub fn export(&self, index: usize) -> Result<Exported> {
        export_surface(&self.dev, self.surfaces[index], ffi::VA_EXPORT_SURFACE_READ_ONLY, "export the RGB surface as dmabuf")
    }

    /// 同一张输出画面再导出一份可写的（着色器转 RGB 时往里画，见 [`crate::GlConverter`]）。
    pub(crate) fn export_writable(&self, index: usize) -> Result<Exported> {
        export_surface(&self.dev, self.surfaces[index], ffi::VA_EXPORT_SURFACE_READ_WRITE, "export the RGB surface for drawing")
    }
}

/// 显卡画面导出成 dmabuf（单层合成排布：打包格式一层一个平面）。
fn export_surface(dev: &Device, surface: ffi::VASurfaceID, access: u32, what: &str) -> Result<Exported> {
    let mut d: ffi::VADRMPRIMESurfaceDescriptor = unsafe { zeroed() };
    va_call!(
        dev,
        vaExportSurfaceHandle(
            dev.display(),
            surface,
            ffi::VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
            access | ffi::VA_EXPORT_SURFACE_COMPOSED_LAYERS,
            &mut d as *mut _ as *mut c_void
        ),
        what
    )?;
    let objects = &d.objects[..d.num_objects as usize];
    Ok(Exported {
        fourcc: d.fourcc,
        width: d.width,
        height: d.height,
        modifiers: objects.iter().map(|o| o.drm_format_modifier).collect(),
        // 每个 fd 交给 OwnedFd，不用时自动关闭
        fds: objects.iter().map(|o| unsafe { OwnedFd::from_raw_fd(o.fd) }).collect(),
        layers: d.layers[..d.num_layers as usize]
            .iter()
            .map(|l| Layer {
                drm_format: l.drm_format,
                planes: (0..l.num_planes as usize)
                    .map(|i| Plane {
                        object: l.object_index[i],
                        offset: l.offset[i],
                        pitch: l.pitch[i],
                    })
                    .collect(),
            })
            .collect(),
    })
}

impl Drop for Converter {
    fn drop(&mut self) {
        let Ok(va) = self.dev.va() else { return };
        let display = self.dev.display();
        unsafe {
            if let (Some(f), true) = (va.vaDestroyContext, self.context != VA_INVALID) {
                f(display, self.context);
            }
            if let (Some(f), false) = (va.vaDestroySurfaces, self.surfaces.is_empty()) {
                f(display, self.surfaces.as_mut_ptr(), self.surfaces.len() as i32);
            }
            if let (Some(f), true) = (va.vaDestroyConfig, self.config != VA_INVALID) {
                f(display, self.config);
            }
        }
    }
}

/// 源画面（`sw`×`sh`）放进目标（`dw`×`dh`）里的位置：等比缩放、居中，返回 (x, y, 宽, 高)；宽高取偶数，
/// 免得驱动对奇数尺寸处理不一。转换和窗口坐标换算都用它，两边对得上。
pub fn fit_rect(sw: u32, sh: u32, dw: u32, dh: u32) -> (u32, u32, u32, u32) {
    let (sw, sh, dw, dh) = (sw.max(1) as u64, sh.max(1) as u64, dw as u64, dh as u64);
    let (w, h) = if sw * dh >= sh * dw {
        (dw, sh * dw / sw)
    } else {
        (sw * dh / sh, dh)
    };
    let (w, h) = (w & !1, h & !1);
    (((dw - w) / 2) as u32, ((dh - h) / 2) as u32, w as u32, h as u32)
}

fn fit(sw: u32, sh: u32, dw: u32, dh: u32) -> ffi::VARectangle {
    let (x, y, width, height) = fit_rect(sw, sh, dw, dh);
    ffi::VARectangle {
        x: x as i16,
        y: y as i16,
        width: width as u16,
        height: height as u16,
    }
}

/// 导出的 dmabuf：DRM 格式、每个内存对象的 fd 和 modifier、各层各平面的位置。
#[derive(Debug)]
pub struct Exported {
    pub fourcc: u32,
    pub width: u32,
    pub height: u32,
    pub modifiers: Vec<u64>,
    pub fds: Vec<OwnedFd>,
    pub layers: Vec<Layer>,
}

impl Exported {
    /// 单平面打包格式（一个内存对象、一层、一个平面）的那个对象、那一层和那个平面；不是这种排布就报错
    pub fn packed_plane(&self) -> Result<(&OwnedFd, &Layer, &Plane)> {
        let ([layer], [fd]) = (self.layers.as_slice(), self.fds.as_slice()) else {
            return Err(Error(format!("{} layer(s) / {} object(s): expected one packed plane", self.layers.len(), self.fds.len())));
        };
        let [plane] = layer.planes.as_slice() else {
            return Err(Error(format!("{} plane(s): expected one packed plane", layer.planes.len())));
        };
        Ok((fd, layer, plane))
    }
}

#[derive(Debug)]
pub struct Layer {
    pub drm_format: u32,
    pub planes: Vec<Plane>,
}

#[derive(Debug)]
pub struct Plane {
    pub object: u32,
    pub offset: u32,
    pub pitch: u32,
}
