//! 显卡着色器转 RGB（OpenGL ES 3，P1 第 7 项第 1 步）：解出的 YUV 4:4:4 画面导出成 dmabuf、直接导入成纹理（不拷贝），
//! 片元着色器按 BT.709 全范围公式算 RGB、显式四舍五入，画进 [`Converter`] 的输出画面（同一组 dmabuf 照旧交给合成器）。
//!
//! 为什么不用视频处理单元：它的精度不受控（8bit 0.5%、10bit 27% 的像素差 1 级），也做不了「这块用原图、那块用视频」的
//! 逐块拼接（静止无损要用）。着色器转换和公式逐像素一致（2026-10-21 瘦客户机实测 8bit / 10bit 都 100%，每帧约 1.75 毫秒）。
//! 这是 mpv、Moonlight、浏览器在 Linux 上显示硬解画面的标准做法。
//!
//! 只做 1:1（解出的画面和输出画面一样大）；要缩放时调用方照旧用视频处理单元。
//! 解出的画面按同样的内存排布「冒充」RGB 格式导入，着色器拿到的就是 Y / U / V 原值：
//! - 8bit：表面是 XYUV（字节依次 V U Y X），按 ABGR8888 导入 → r = V、g = U、b = Y；
//! - 10bit：表面是 Y410（低位起 U10 Y10 V10 A2），按 XRGB2101010 导入 → b = U、g = Y、r = V。
//!
//! EGL 用 GBM 平台开在显卡的渲染节点上（不要桌面、不要窗口，只往离屏画面里画）；上下文只在创建它的线程上用。
//! 只认一块显卡：渲染节点取 /dev/dri/renderD128，和 VAAPI 默认打开的是同一块（多显卡主控端以后再说）。

use crate::vaapi::{Converter, Decoder, Error, Exported, Frame, Layer, Plane, Result};
use glow::HasContext;
use khronos_egl as egl;
use std::{
    collections::HashMap,
    ffi::c_void,
    fs::{File, OpenOptions},
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
};

const RENDER_NODE: &str = "/dev/dri/renderD128";
const PLATFORM_GBM_KHR: egl::Enum = 0x31D7;
const LINUX_DMA_BUF_EXT: egl::Enum = 0x3270;
const LINUX_DRM_FOURCC_EXT: egl::Attrib = 0x3271;
const DMA_BUF_PLANE0_FD_EXT: egl::Attrib = 0x3272;
const DMA_BUF_PLANE0_OFFSET_EXT: egl::Attrib = 0x3273;
const DMA_BUF_PLANE0_PITCH_EXT: egl::Attrib = 0x3274;
const DMA_BUF_PLANE0_MODIFIER_LO_EXT: egl::Attrib = 0x3443;
const DMA_BUF_PLANE0_MODIFIER_HI_EXT: egl::Attrib = 0x3444;
/// 修饰符里的「无效」：导出方没给修饰符时不传，让驱动按默认排布理解
const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

const fn fourcc(s: &[u8; 4]) -> u32 {
    s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16 | (s[3] as u32) << 24
}
const DRM_XYUV8888: u32 = fourcc(b"XYUV");
const DRM_Y410: u32 = fourcc(b"Y410");
const DRM_ABGR8888: u32 = fourcc(b"AB24");
const DRM_XRGB2101010: u32 = fourcc(b"XR30");
const DRM_XRGB8888: u32 = fourcc(b"XR24");

const VERTEX: &str = "#version 300 es
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}";

/// 公式和被控端颜色转换着色器（ddpro-enc 的 rgb_to_yuv444.hlsl）互逆：BT.709、全范围。
/// 最后显式四舍五入到 8 位再交出去——交给显卡自己从浮点写成 8 位时，实测 0.33% 的像素差 1 级。
/// 静止无损（`tile` > 0）：掩码里置了的方块直接取无损纹理里的原始像素，不用视频
///
/// HDR（`hdr` = 1，P1 第 4 项）：和被控端 scrgb_to_pq444_10.hlsl 互逆——BT.2020 非恒定亮度矩阵还原 R'G'B'、PQ 还原成尼特，
/// 再除以被控端「普通内容的白」（`sdr_white`，尼特）→ 1.0 就是普通内容的白；BT.2020 转 BT.709 原色；比白亮的高光按最大的那个
/// 通道整体缩回 1.0（保色相，不像按通道截那样偏色）；sRGB 编码。普通内容因此和被控端没开 HDR 时一样，高光截到白——
/// 显示器没有 HDR 输出，没地方放比白还亮的东西，宁可截高光也不把普通内容的白压灰（专业模式先保普通内容的颜色）
const FRAGMENT: &str = "#version 300 es
precision highp float;
uniform highp sampler2D yuv;
uniform highp sampler2D lossless;
uniform highp sampler2D mask;
uniform int ten;
uniform int tile;
uniform int hdr;
uniform float sdr_white;
out vec4 o;
const mat3 BT2020_TO_709 = mat3(1.660491, -0.124550, -0.018151, -0.587641, 1.132900, -0.100579, -0.072850, -0.008349, 1.118730);
void main() {
    ivec2 p = ivec2(gl_FragCoord.xy);
    if (tile > 0 && texelFetch(mask, p / tile, 0).r > 0.5) {
        o = vec4(floor(texelFetch(lossless, p, 0).rgb * 255.0 + 0.5) / 255.0, 1.0);
        return;
    }
    vec4 s = texelFetch(yuv, p, 0);
    float peak = ten == 1 ? 1023.0 : 255.0;
    float half_ = ten == 1 ? 512.0 : 128.0;
    float y = ten == 1 ? s.g : s.b;
    float u = ten == 1 ? s.b : s.g;
    float v = s.r;
    float cb = (u * peak - half_) / peak;
    float cr = (v * peak - half_) / peak;
    if (hdr == 1) {
        float r2 = y + 1.4746 * cr;
        float b2 = y + 1.8814 * cb;
        vec3 e = clamp(vec3(r2, (y - 0.2627 * r2 - 0.0593 * b2) / 0.6780, b2), 0.0, 1.0);
        vec3 q = pow(e, vec3(1.0 / 78.84375));
        vec3 nits = 10000.0 * pow(max(q - 0.8359375, 0.0) / (18.8515625 - 18.6875 * q), vec3(1.0 / 0.1593017578125));
        vec3 lin = max(BT2020_TO_709 * (nits / sdr_white), 0.0);
        float m = max(max(lin.r, lin.g), lin.b);
        if (m > 1.0) {
            lin /= m;
        }
        vec3 srgb = mix(1.055 * pow(lin, vec3(1.0 / 2.4)) - 0.055, lin * 12.92, lessThanEqual(lin, vec3(0.0031308)));
        o = vec4(floor(clamp(srgb, 0.0, 1.0) * 255.0 + 0.5) / 255.0, 1.0);
        return;
    }
    float r = y + 1.5748 * cr;
    float b = y + 1.8556 * cb;
    float g = (y - 0.2126 * r - 0.0722 * b) / 0.7152;
    o = vec4(floor(clamp(vec3(r, g, b), 0.0, 1.0) * 255.0 + 0.5) / 255.0, 1.0);
}";

type ImageTargetFn = unsafe extern "system" fn(target: u32, image: *const c_void);
type Egl = egl::DynamicInstance<egl::EGL1_5>;

fn err(what: &str) -> impl Fn(egl::Error) -> Error + '_ {
    move |e| Error(format!("{what}: {e}"))
}

/// libgbm 运行时打开（Mesa 自带）：建设备、分配 / 导出 / 释放画面。
struct Gbm {
    lib: libloading::Library,
    dev: *mut c_void,
    _node: File,
}

/// GBM 分配的一张画面
struct Bo(*mut c_void);

/// gbm_bo_create_with_modifiers2 的用途标记：要送显示、要往里画
const GBM_BO_USE_SCANOUT: u32 = 1 << 0;
const GBM_BO_USE_RENDERING: u32 = 1 << 2;

impl Gbm {
    unsafe fn sym<T>(&self, name: &[u8]) -> Result<libloading::Symbol<'_, T>> {
        self.lib
            .get(name)
            .map_err(|e| Error(format!("libgbm: {e}")))
    }

    /// 按 `modifiers` 里驱动支持的一种排布分配一张 XRGB8888 画面（驱动自己挑）
    fn alloc(&self, width: u32, height: u32, modifiers: &[u64]) -> Result<Bo> {
        unsafe {
            let create = self.sym::<unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const u64, u32, u32) -> *mut c_void>(
                b"gbm_bo_create_with_modifiers2\0",
            )?;
            let bo = create(
                self.dev,
                width,
                height,
                DRM_XRGB8888,
                modifiers.as_ptr(),
                modifiers.len() as u32,
                GBM_BO_USE_SCANOUT | GBM_BO_USE_RENDERING,
            );
            if bo.is_null() {
                return Err(Error(format!("GBM cannot allocate a {width}x{height} XRGB8888 image with modifiers {modifiers:x?}")));
            }
            Ok(Bo(bo))
        }
    }

    /// 导出成 dmabuf（每次一个新 fd）
    fn export(&self, bo: &Bo, width: u32, height: u32) -> Result<Exported> {
        unsafe {
            let fd = self.sym::<unsafe extern "C" fn(*mut c_void) -> RawFd>(b"gbm_bo_get_fd\0")?(bo.0);
            if fd < 0 {
                return Err(Error("gbm_bo_get_fd failed".into()));
            }
            let fd = OwnedFd::from_raw_fd(fd);
            let pitch = self.sym::<unsafe extern "C" fn(*mut c_void) -> u32>(b"gbm_bo_get_stride\0")?(bo.0);
            let modifier = self.sym::<unsafe extern "C" fn(*mut c_void) -> u64>(b"gbm_bo_get_modifier\0")?(bo.0);
            Ok(Exported {
                fourcc: DRM_XRGB8888,
                width,
                height,
                modifiers: vec![modifier],
                fds: vec![fd],
                layers: vec![Layer {
                    drm_format: DRM_XRGB8888,
                    planes: vec![Plane { object: 0, offset: 0, pitch }],
                }],
            })
        }
    }

    fn free(&self, bo: Bo) {
        if let Ok(destroy) = unsafe { self.sym::<unsafe extern "C" fn(*mut c_void)>(b"gbm_bo_destroy\0") } {
            unsafe { destroy(bo.0) };
        }
    }

    fn open() -> Result<Self> {
        let node = OpenOptions::new()
            .read(true)
            .write(true)
            .open(RENDER_NODE)
            .map_err(|e| Error(format!("open {RENDER_NODE}: {e}")))?;
        unsafe {
            let lib = libloading::Library::new("libgbm.so.1").map_err(|e| Error(format!("load libgbm: {e}")))?;
            let create: libloading::Symbol<unsafe extern "C" fn(RawFd) -> *mut c_void> =
                lib.get(b"gbm_create_device\0").map_err(|e| Error(format!("libgbm: {e}")))?;
            let dev = create(node.as_raw_fd());
            if dev.is_null() {
                return Err(Error(format!("gbm_create_device on {RENDER_NODE} failed")));
            }
            Ok(Self { lib, dev, _node: node })
        }
    }
}

impl Drop for Gbm {
    fn drop(&mut self) {
        unsafe {
            if let Ok(destroy) = self.lib.get::<unsafe extern "C" fn(*mut c_void)>(b"gbm_device_destroy\0") {
                destroy(self.dev);
            }
        }
    }
}

struct Target {
    image: egl::Image,
    texture: glow::Texture,
    fbo: glow::Framebuffer,
}

struct Source {
    image: egl::Image,
    texture: glow::Texture,
    ten: bool,
}

/// 着色器转换器。输出画面有两种来源：
/// - [`GlConverter::new`]：自己用 GBM 按给的排布分配（专业窗口优先这样：分块排布着色器画得快、合成器能直接送显示），
///   用 [`GlConverter::export`] 交给窗口显示、交给 [`Converter::with_images`] 让视频处理单元在要缩放时也画进同一组；
/// - [`GlConverter::on`]：画进 [`Converter`] 自己建的输出画面（上一种建不起来时的退路）。
pub struct GlConverter {
    egl: Egl,
    display: egl::Display,
    context: egl::Context,
    gl: glow::Context,
    image_target: ImageTargetFn,
    program: glow::Program,
    vao: glow::VertexArray,
    ten: Option<glow::UniformLocation>,
    tile: Option<glow::UniformLocation>,
    /// 静止无损：原始像素（和输出画面一样大，第一次放方块时建）、方块掩码（每块一个字节）和方块边长（0 = 不用）
    lossless: Option<glow::Texture>,
    mask: Option<(glow::Texture, u32, u32)>,
    tile_size: u32,
    targets: Vec<Target>,
    width: u32,
    height: u32,
    /// 解码器画面池里每张画面导入一次就留着（按画面编号）；画面池重建时调用方用 [`GlConverter::forget_sources`] 清掉
    sources: HashMap<u32, Source>,
    renderer: String,
    /// 自己分配的输出画面（[`GlConverter::new`]），和 `targets` 一一对应；[`GlConverter::on`] 时为空
    bos: Vec<Bo>,
    // 最后析构：EGL 显示和上下文、GBM 画面都建在它上面
    gbm: Gbm,
}

impl GlConverter {
    /// 自己用 GBM 分配 `count` 张输出画面（排布从 `modifiers` 里由驱动挑），着色器往里画。
    /// 任何一步不支持就报错，调用方退回 [`GlConverter::on`] 或视频处理单元。
    pub fn new(width: u32, height: u32, count: usize, modifiers: &[u64]) -> Result<Self> {
        let mut c = Self::init(width, height)?;
        for _ in 0..count {
            let bo = c.gbm.alloc(width, height, modifiers)?;
            let e = c.gbm.export(&bo, width, height);
            c.bos.push(bo);
            let t = c.target(&e?)?;
            c.targets.push(t);
        }
        Ok(c)
    }

    /// 画进 `conv` 自己建的那几张输出画面。
    pub fn on(conv: &Converter) -> Result<Self> {
        let (width, height) = conv.size();
        let mut c = Self::init(width, height)?;
        for i in 0..conv.count() {
            let e = conv.export_writable(i)?;
            let t = c.target(&e)?;
            c.targets.push(t);
        }
        Ok(c)
    }

    /// 自己分配的第 `index` 张输出画面导出成 dmabuf（每次一个新 fd）：交给窗口显示、交给视频处理单元导入
    pub fn export(&self, index: usize) -> Result<Exported> {
        let bo = self
            .bos
            .get(index)
            .ok_or_else(|| Error(format!("no output image {index} of my own")))?;
        self.gbm.export(bo, self.width, self.height)
    }

    /// EGL、上下文、着色器；还没有输出画面
    fn init(width: u32, height: u32) -> Result<Self> {
        let gbm = Gbm::open()?;
        let egl = unsafe { Egl::load_required() }.map_err(|e| Error(format!("load libEGL: {e}")))?;
        let display = unsafe { egl.get_platform_display(PLATFORM_GBM_KHR, gbm.dev, &[egl::ATTRIB_NONE]) }
            .map_err(err("open the EGL display"))?;
        egl.initialize(display).map_err(err("initialize EGL"))?;
        // 这之后出错要把上下文和显示放掉再返回：EGL 显示按 GBM 设备地址认，调用方紧接着换一种做法新开的 GBM 设备
        // 很可能落在同一地址，会拿到这个仍处于「已初始化」、底下的设备却已释放的显示
        let mut created = None;
        let rest = (|| {
            let ext = egl.query_string(Some(display), egl::EXTENSIONS).map_err(err("query EGL extensions"))?;
            let ext = ext.to_string_lossy();
            for need in ["EGL_EXT_image_dma_buf_import", "EGL_EXT_image_dma_buf_import_modifiers", "EGL_KHR_surfaceless_context", "EGL_KHR_no_config_context"] {
                if !ext.split(' ').any(|e| e == need) {
                    return Err(Error(format!("EGL lacks {need}")));
                }
            }
            egl.bind_api(egl::OPENGL_ES_API).map_err(err("bind OpenGL ES"))?;
            let no_config = unsafe { egl::Config::from_ptr(std::ptr::null_mut()) };
            let context = egl
                .create_context(display, no_config, None, &[egl::CONTEXT_MAJOR_VERSION, 3, egl::NONE])
                .map_err(err("create the OpenGL ES 3 context"))?;
            created = Some(context);
            egl.make_current(display, None, None, Some(context)).map_err(err("make the context current"))?;
            let gl = unsafe {
                glow::Context::from_loader_function(|s| egl.get_proc_address(s).map_or(std::ptr::null(), |p| p as *const c_void))
            };
            let image_target = egl
                .get_proc_address("glEGLImageTargetTexture2DOES")
                .ok_or_else(|| Error("no glEGLImageTargetTexture2DOES".into()))?;
            let image_target: ImageTargetFn = unsafe { std::mem::transmute(image_target) };
            let renderer = unsafe { gl.get_parameter_string(glow::RENDERER) };
            let (program, vao, ten, tile) = unsafe { build_program(&gl)? };
            Ok((context, gl, image_target, renderer, program, vao, ten, tile))
        })();
        let (context, gl, image_target, renderer, program, vao, ten, tile) = match rest {
            Ok(v) => v,
            Err(e) => {
                egl.make_current(display, None, None, None).ok();
                if let Some(c) = created {
                    egl.destroy_context(display, c).ok();
                }
                egl.terminate(display).ok();
                return Err(e);
            }
        };
        Ok(Self {
            egl,
            display,
            context,
            gl,
            image_target,
            program,
            vao,
            ten,
            tile,
            lossless: None,
            mask: None,
            tile_size: 0,
            targets: Vec::new(),
            width,
            height,
            sources: HashMap::new(),
            renderer,
            bos: Vec::new(),
            gbm,
        })
    }

    /// 显卡名（日志用）
    /// HDR 画面（P1 第 4 项）：`Some(被控端普通内容的白，尼特)` 时按 BT.2020 + PQ 还原、高光截到白；`None` 是普通画面（默认）。
    /// 着色器参数是程序的状态，设一次一直有效
    pub fn set_hdr(&mut self, sdr_white_nits: Option<u32>) -> Result<()> {
        self.egl
            .make_current(self.display, None, None, Some(self.context))
            .map_err(err("make the context current"))?;
        let gl = &self.gl;
        unsafe {
            gl.use_program(Some(self.program));
            let hdr = gl.get_uniform_location(self.program, "hdr");
            gl.uniform_1_i32(hdr.as_ref(), sdr_white_nits.is_some() as i32);
            let white = gl.get_uniform_location(self.program, "sdr_white");
            gl.uniform_1_f32(white.as_ref(), sdr_white_nits.unwrap_or(80).max(1) as f32);
        }
        Ok(())
    }

    pub fn renderer(&self) -> &str {
        &self.renderer
    }

    /// 从着色器这一侧读回第 `index` 张输出画面，按 R G B 逐像素、从上到下排好（验证用，正式显示不走这条路）。
    pub fn read_rgb(&self, index: usize) -> Result<Vec<u8>> {
        let t = self.targets.get(index).ok_or_else(|| Error(format!("no output image {index}")))?;
        self.egl
            .make_current(self.display, None, None, Some(self.context))
            .map_err(err("make the context current"))?;
        let (w, h) = (self.width as usize, self.height as usize);
        let mut rgba = vec![0u8; w * h * 4];
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.fbo));
            self.gl.read_pixels(
                0,
                0,
                w as i32,
                h as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut rgba)),
            );
        }
        Ok(rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect())
    }

    /// 把 `frame` 转成 RGB 画进第 `index` 张输出画面，等显卡画完再返回。只做 1:1：尺寸和输出画面不一样就报错。
    pub fn convert(&mut self, dec: &Decoder, frame: &Frame, index: usize) -> Result<()> {
        let size = (frame.width(), frame.height());
        if size != (self.width, self.height) {
            return Err(Error(format!("{}x{} frame on a {}x{} output: shaders only convert 1:1", size.0, size.1, self.width, self.height)));
        }
        // 换尺寸时新转换器先建、旧的后析构，旧的析构会把线程的当前上下文清掉：每次都切回自己的（已是当前的话驱动不做事）
        self.egl
            .make_current(self.display, None, None, Some(self.context))
            .map_err(err("make the context current"))?;
        let id = frame.surface_id();
        if !self.sources.contains_key(&id) {
            let e = dec.export(frame)?;
            let s = self.source(&e)?;
            self.sources.insert(id, s);
        }
        let (Some(src), Some(dst)) = (self.sources.get(&id), self.targets.get(index)) else {
            return Err(Error(format!("no output image {index}")));
        };
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(dst.fbo));
            gl.viewport(0, 0, self.width as i32, self.height as i32);
            gl.use_program(Some(self.program));
            gl.uniform_1_i32(self.ten.as_ref(), src.ten as i32);
            let tile = if self.mask.is_some() && self.lossless.is_some() { self.tile_size } else { 0 };
            gl.uniform_1_i32(self.tile.as_ref(), tile as i32);
            if tile > 0 {
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, self.lossless);
                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D, self.mask.map(|m| m.0));
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(src.texture));
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.finish();
            match gl.get_error() {
                glow::NO_ERROR => Ok(()),
                e => Err(Error(format!("OpenGL error {e:#x} converting a frame"))),
            }
        }
    }

    /// 静止无损：把一块原始像素（RGBA，逐行紧排，从上到下）放进无损纹理的 (x, y) 处。掩码里置上这块之后画出来才用它。
    pub fn put_tile(&mut self, x: u32, y: u32, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
        if x + w > self.width || y + h > self.height || rgba.len() != (w * h * 4) as usize {
            return Err(Error(format!("tile {w}x{h} at ({x},{y}) with {} bytes on a {}x{} image", rgba.len(), self.width, self.height)));
        }
        self.egl
            .make_current(self.display, None, None, Some(self.context))
            .map_err(err("make the context current"))?;
        let gl = &self.gl;
        unsafe {
            let t = match self.lossless {
                Some(t) => t,
                None => {
                    let t = gl.create_texture().map_err(Error)?;
                    gl.bind_texture(glow::TEXTURE_2D, Some(t));
                    gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, self.width as i32, self.height as i32);
                    self.lossless = Some(t);
                    t
                }
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                x as i32,
                y as i32,
                w as i32,
                h as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(rgba)),
            );
            match gl.get_error() {
                glow::NO_ERROR => Ok(()),
                e => Err(Error(format!("OpenGL error {e:#x} uploading a lossless tile"))),
            }
        }
    }

    /// 静止无损的方块掩码：`tile` 像素一块、`cols`×`rows` 块，每块一个字节（非 0 = 这块用无损纹理）。`tile` 为 0 时不用无损。
    pub fn set_mask(&mut self, tile: u32, cols: u32, rows: u32, mask: &[u8]) -> Result<()> {
        if tile > 0 && mask.len() != (cols * rows) as usize {
            return Err(Error(format!("mask of {} bytes for {cols}x{rows} tiles", mask.len())));
        }
        // 新掩码传上去之前不用无损：上传失败时显卡里还是旧掩码，照它画会让已经变了的块显示旧图
        self.tile_size = 0;
        if tile == 0 {
            return Ok(());
        }
        self.egl
            .make_current(self.display, None, None, Some(self.context))
            .map_err(err("make the context current"))?;
        let gl = &self.gl;
        unsafe {
            let t = match self.mask {
                Some((t, c, r)) if (c, r) == (cols, rows) => t,
                old => {
                    if let Some((t, ..)) = old {
                        gl.delete_texture(t);
                    }
                    let t = gl.create_texture().map_err(Error)?;
                    gl.bind_texture(glow::TEXTURE_2D, Some(t));
                    gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::R8, cols as i32, rows as i32);
                    self.mask = Some((t, cols, rows));
                    t
                }
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                cols as i32,
                rows as i32,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(mask)),
            );
            match gl.get_error() {
                glow::NO_ERROR => {
                    self.tile_size = tile;
                    Ok(())
                }
                e => Err(Error(format!("OpenGL error {e:#x} uploading the lossless mask"))),
            }
        }
    }

    /// 一张 dmabuf 导入成 EGL 图像（只认单平面的打包格式，`as_fourcc` 是按什么格式理解它的内存）
    fn import(&self, e: &Exported, as_fourcc: u32) -> Result<egl::Image> {
        let ([layer], [fd]) = (e.layers.as_slice(), e.fds.as_slice()) else {
            return Err(Error(format!("{} layer(s) / {} object(s): expected one packed plane", e.layers.len(), e.fds.len())));
        };
        let [plane] = layer.planes.as_slice() else {
            return Err(Error(format!("{} plane(s): expected one packed plane", layer.planes.len())));
        };
        let modifier = e.modifiers.first().copied().unwrap_or(DRM_FORMAT_MOD_INVALID);
        let mut attrs = vec![
            egl::WIDTH as egl::Attrib,
            e.width as egl::Attrib,
            egl::HEIGHT as egl::Attrib,
            e.height as egl::Attrib,
            LINUX_DRM_FOURCC_EXT,
            as_fourcc as egl::Attrib,
            DMA_BUF_PLANE0_FD_EXT,
            fd.as_raw_fd() as egl::Attrib,
            DMA_BUF_PLANE0_OFFSET_EXT,
            plane.offset as egl::Attrib,
            DMA_BUF_PLANE0_PITCH_EXT,
            plane.pitch as egl::Attrib,
        ];
        if modifier != DRM_FORMAT_MOD_INVALID {
            attrs.extend([
                DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                (modifier & 0xffff_ffff) as egl::Attrib,
                DMA_BUF_PLANE0_MODIFIER_HI_EXT,
                (modifier >> 32) as egl::Attrib,
            ]);
        }
        attrs.push(egl::ATTRIB_NONE);
        let no_context = unsafe { egl::Context::from_ptr(egl::NO_CONTEXT) };
        let no_buffer = unsafe { egl::ClientBuffer::from_ptr(std::ptr::null_mut()) };
        self.egl
            .create_image(self.display, no_context, LINUX_DMA_BUF_EXT, no_buffer, &attrs)
            .map_err(err("import a dmabuf"))
    }

    /// 导入的图像绑成纹理（逐像素取，不插值）
    unsafe fn texture(&self, image: &egl::Image) -> Result<glow::Texture> {
        let gl = &self.gl;
        let t = gl.create_texture().map_err(Error)?;
        gl.bind_texture(glow::TEXTURE_2D, Some(t));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        (self.image_target)(glow::TEXTURE_2D, image.as_ptr() as *const c_void);
        match gl.get_error() {
            glow::NO_ERROR => Ok(t),
            e => {
                gl.delete_texture(t);
                Err(Error(format!("bind a dmabuf as a texture: OpenGL error {e:#x}")))
            }
        }
    }

    fn target(&self, e: &Exported) -> Result<Target> {
        let format = e.layers.first().map(|l| l.drm_format).unwrap_or_default();
        if format != DRM_XRGB8888 {
            return Err(Error(format!("output image format {:#x}, expected XRGB8888", format)));
        }
        let image = self.import(e, DRM_XRGB8888)?;
        unsafe {
            let texture = self.texture(&image).inspect_err(|_| {
                self.egl.destroy_image(self.display, image).ok();
            })?;
            let gl = &self.gl;
            let fbo = gl.create_framebuffer().map_err(Error)?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(fbo);
                gl.delete_texture(texture);
                self.egl.destroy_image(self.display, image).ok();
                return Err(Error("the output image cannot be drawn into".into()));
            }
            Ok(Target { image, texture, fbo })
        }
    }

    fn source(&self, e: &Exported) -> Result<Source> {
        let format = e.layers.first().map(|l| l.drm_format).unwrap_or_default();
        let (as_fourcc, ten) = match format {
            DRM_XYUV8888 => (DRM_ABGR8888, false),
            DRM_Y410 => (DRM_XRGB2101010, true),
            f => return Err(Error(format!("decoded surface format {f:#x} is not XYUV / Y410"))),
        };
        let image = self.import(e, as_fourcc)?;
        let texture = unsafe { self.texture(&image) }.inspect_err(|_| {
            self.egl.destroy_image(self.display, image).ok();
        })?;
        Ok(Source { image, texture, ten })
    }

    /// 解码画面的尺寸变了（解码器重建了画面池，画面编号会被新画面复用）：按编号缓存的导入全部作废，下次用到时重新导入。
    /// 只做 1:1 的 [`GlConverter::convert`] 自己看不到别的尺寸的画面（那些走视频处理单元），所以由调用方在尺寸变化时调。
    pub fn forget_sources(&mut self) {
        if self.sources.is_empty() {
            return;
        }
        if self.egl.make_current(self.display, None, None, Some(self.context)).is_ok() {
            self.drop_sources();
        }
    }

    fn drop_sources(&mut self) {
        for (_, s) in self.sources.drain() {
            unsafe { self.gl.delete_texture(s.texture) };
            self.egl.destroy_image(self.display, s.image).ok();
        }
    }
}

type Uniform = Option<glow::UniformLocation>;

unsafe fn build_program(gl: &glow::Context) -> Result<(glow::Program, glow::VertexArray, Uniform, Uniform)> {
    let program = gl.create_program().map_err(Error)?;
    for (kind, src) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
        let s = gl.create_shader(kind).map_err(Error)?;
        gl.shader_source(s, src);
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            let log = gl.get_shader_info_log(s);
            gl.delete_shader(s);
            gl.delete_program(program);
            return Err(Error(format!("compile the conversion shader: {log}")));
        }
        gl.attach_shader(program, s);
        gl.delete_shader(s);
    }
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(Error(format!("link the conversion shader: {log}")));
    }
    gl.use_program(Some(program));
    for (name, unit) in [("yuv", 0), ("lossless", 1), ("mask", 2)] {
        if let Some(l) = gl.get_uniform_location(program, name) {
            gl.uniform_1_i32(Some(&l), unit);
        }
    }
    let ten = gl.get_uniform_location(program, "ten");
    let tile = gl.get_uniform_location(program, "tile");
    let vao = gl.create_vertex_array().map_err(Error)?;
    Ok((program, vao, ten, tile))
}

impl Drop for GlConverter {
    fn drop(&mut self) {
        // 删 GL 对象要在它们自己的上下文上
        self.egl.make_current(self.display, None, None, Some(self.context)).ok();
        self.drop_sources();
        unsafe {
            for t in self.targets.drain(..) {
                self.gl.delete_framebuffer(t.fbo);
                self.gl.delete_texture(t.texture);
                self.egl.destroy_image(self.display, t.image).ok();
            }
            if let Some(t) = self.lossless.take() {
                self.gl.delete_texture(t);
            }
            if let Some((t, ..)) = self.mask.take() {
                self.gl.delete_texture(t);
            }
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_program(self.program);
        }
        self.egl.make_current(self.display, None, None, None).ok();
        self.egl.destroy_context(self.display, self.context).ok();
        self.egl.terminate(self.display).ok();
        // 窗口和视频处理单元拿的是导出的 fd，各自持有引用；这里只放掉自己的
        for bo in self.bos.drain(..) {
            self.gbm.free(bo);
        }
    }
}
