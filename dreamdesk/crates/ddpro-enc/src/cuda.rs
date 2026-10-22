//! CUDA 驱动 API 的最小绑定（显卡驱动自带的 nvcuda.dll，不需要 CUDA SDK）：只为 4:4:4 10bit 编码，
//! 把颜色转换着色器输出的 D3D11 纹理每帧拷进 CUDA 缓冲区交给 NVENC（P1 第 4 项）。
//! NVENC 在 D3D11 下不收 4:4:4 10bit 的画面，要开 CUDA 类型的编码会话（Sunshine 也是这么做的）。

use crate::Error;
use std::{ffi::c_void, mem, ptr};
use windows::{
    core::{s, Interface},
    Win32::{
        Graphics::{
            Direct3D11::{ID3D11Device, ID3D11Texture2D},
            Dxgi::{IDXGIAdapter, IDXGIDevice},
        },
        System::LibraryLoader::{GetProcAddress, LoadLibraryA},
    },
};

type CuResult = i32;
pub type CuContext = *mut c_void;
pub type CuDevicePtr = u64;
type CuGraphicsResource = *mut c_void;
type CuArray = *mut c_void;

const CU_MEMORYTYPE_DEVICE: u32 = 2;
const CU_MEMORYTYPE_ARRAY: u32 = 3;
const CU_GRAPHICS_MAP_RESOURCE_FLAGS_READ_ONLY: u32 = 1;

/// `CUDA_MEMCPY2D`（v2）
#[repr(C)]
struct Memcpy2d {
    src_x_in_bytes: usize,
    src_y: usize,
    src_memory_type: u32,
    src_host: *const c_void,
    src_device: CuDevicePtr,
    src_array: CuArray,
    src_pitch: usize,
    dst_x_in_bytes: usize,
    dst_y: usize,
    dst_memory_type: u32,
    dst_host: *mut c_void,
    dst_device: CuDevicePtr,
    dst_array: CuArray,
    dst_pitch: usize,
    width_in_bytes: usize,
    height: usize,
}

/// 驱动的函数表（只有函数指针，复制一份用不必借）。dll 加载后不卸载
#[derive(Clone, Copy)]
struct Api {
    init: unsafe extern "system" fn(u32) -> CuResult,
    d3d11_get_device: unsafe extern "system" fn(*mut i32, *mut c_void) -> CuResult,
    ctx_create: unsafe extern "system" fn(*mut CuContext, u32, i32) -> CuResult,
    ctx_destroy: unsafe extern "system" fn(CuContext) -> CuResult,
    ctx_push: unsafe extern "system" fn(CuContext) -> CuResult,
    ctx_pop: unsafe extern "system" fn(*mut CuContext) -> CuResult,
    mem_alloc_pitch: unsafe extern "system" fn(*mut CuDevicePtr, *mut usize, usize, usize, u32) -> CuResult,
    mem_free: unsafe extern "system" fn(CuDevicePtr) -> CuResult,
    register: unsafe extern "system" fn(*mut CuGraphicsResource, *mut c_void, u32) -> CuResult,
    unregister: unsafe extern "system" fn(CuGraphicsResource) -> CuResult,
    set_map_flags: unsafe extern "system" fn(CuGraphicsResource, u32) -> CuResult,
    map: unsafe extern "system" fn(u32, *mut CuGraphicsResource, *mut c_void) -> CuResult,
    unmap: unsafe extern "system" fn(u32, *mut CuGraphicsResource, *mut c_void) -> CuResult,
    mapped_array: unsafe extern "system" fn(*mut CuArray, CuGraphicsResource, u32, u32) -> CuResult,
    memcpy_2d: unsafe extern "system" fn(*const Memcpy2d) -> CuResult,
    error_string: unsafe extern "system" fn(CuResult, *mut *const i8) -> CuResult,
}

impl Api {
    fn load() -> Result<Self, Error> {
        unsafe {
            let lib = LoadLibraryA(s!("nvcuda.dll"))
                .map_err(|_| Error::Unsupported("nvcuda.dll not found (no NVIDIA driver)".to_owned()))?;
            macro_rules! sym {
                ($name:literal) => {
                    mem::transmute(
                        GetProcAddress(lib, s!($name))
                            .ok_or_else(|| Error::Unsupported(concat!("nvcuda.dll has no ", $name).to_owned()))?,
                    )
                };
            }
            Ok(Self {
                init: sym!("cuInit"),
                d3d11_get_device: sym!("cuD3D11GetDevice"),
                ctx_create: sym!("cuCtxCreate_v2"),
                ctx_destroy: sym!("cuCtxDestroy_v2"),
                ctx_push: sym!("cuCtxPushCurrent_v2"),
                ctx_pop: sym!("cuCtxPopCurrent_v2"),
                mem_alloc_pitch: sym!("cuMemAllocPitch_v2"),
                mem_free: sym!("cuMemFree_v2"),
                register: sym!("cuGraphicsD3D11RegisterResource"),
                unregister: sym!("cuGraphicsUnregisterResource"),
                set_map_flags: sym!("cuGraphicsResourceSetMapFlags_v2"),
                map: sym!("cuGraphicsMapResources"),
                unmap: sym!("cuGraphicsUnmapResources"),
                mapped_array: sym!("cuGraphicsSubResourceGetMappedArray"),
                memcpy_2d: sym!("cuMemcpy2D_v2"),
                error_string: sym!("cuGetErrorString"),
            })
        }
    }

    fn check(&self, r: CuResult, call: &'static str) -> Result<(), Error> {
        if r == 0 {
            return Ok(());
        }
        let mut s: *const i8 = ptr::null();
        let detail = unsafe {
            if (self.error_string)(r, &mut s) == 0 && !s.is_null() {
                std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned()
            } else {
                String::new()
            }
        };
        Err(Error::Cuda { call, status: r, detail })
    }
}

/// 一块 D3D11 纹理到一块 CUDA 缓冲区的每帧拷贝：CUDA 上下文建在 D3D11 设备所在的那块显卡上。
/// NVENC 的 CUDA 编码会话用 [`Self::context`]，输入注册 [`Self::buffer`]（按 [`Self::pitch`] 一行）。
pub struct TextureCopy {
    api: Api,
    ctx: CuContext,
    resource: CuGraphicsResource,
    buffer: CuDevicePtr,
    pitch: usize,
    width_bytes: usize,
    rows: usize,
}

impl TextureCopy {
    /// `texture` 每行 `width_bytes` 字节、共 `rows` 行（R16 纹理：宽 × 2 字节）
    pub fn new(device: &ID3D11Device, texture: &ID3D11Texture2D, width_bytes: usize, rows: usize) -> Result<Self, Error> {
        let api = Api::load()?;
        unsafe {
            api.check((api.init)(0), "cuInit")?;
            let adapter: IDXGIAdapter = device.cast::<IDXGIDevice>()?.GetAdapter()?;
            let mut dev = 0;
            api.check((api.d3d11_get_device)(&mut dev, adapter.as_raw()), "cuD3D11GetDevice")?;
            let mut ctx = ptr::null_mut();
            api.check((api.ctx_create)(&mut ctx, 0, dev), "cuCtxCreate")?;
            // cuCtxCreate 会把新上下文设成本线程当前的：弹出来，用的时候再推
            let mut popped = ptr::null_mut();
            (api.ctx_pop)(&mut popped);
            let mut this = Self { api, ctx, resource: ptr::null_mut(), buffer: 0, pitch: 0, width_bytes, rows };
            this.with_context(|api, this| {
                api.check((api.register)(&mut this.resource, texture.as_raw(), 0), "cuGraphicsD3D11RegisterResource")?;
                api.check(
                    (api.set_map_flags)(this.resource, CU_GRAPHICS_MAP_RESOURCE_FLAGS_READ_ONLY),
                    "cuGraphicsResourceSetMapFlags",
                )?;
                let (mut buf, mut pitch) = (0, 0);
                api.check((api.mem_alloc_pitch)(&mut buf, &mut pitch, width_bytes, rows, 16), "cuMemAllocPitch")?;
                this.buffer = buf;
                this.pitch = pitch;
                Ok(())
            })?;
            Ok(this)
        }
    }

    pub fn context(&self) -> CuContext {
        self.ctx
    }

    pub fn buffer(&self) -> CuDevicePtr {
        self.buffer
    }

    pub fn pitch(&self) -> usize {
        self.pitch
    }

    /// 把纹理当前的内容拷进缓冲区。映射纹理时 CUDA 会等之前提交给 D3D11 的绘制（颜色转换）做完
    pub fn copy(&mut self) -> Result<(), Error> {
        self.with_context(|api, this| unsafe {
            api.check((api.map)(1, &mut this.resource, ptr::null_mut()), "cuGraphicsMapResources")?;
            let mut array = ptr::null_mut();
            let mut r = (api.mapped_array)(&mut array, this.resource, 0, 0);
            if r == 0 {
                let p = Memcpy2d {
                    src_x_in_bytes: 0,
                    src_y: 0,
                    src_memory_type: CU_MEMORYTYPE_ARRAY,
                    src_host: ptr::null(),
                    src_device: 0,
                    src_array: array,
                    src_pitch: 0,
                    dst_x_in_bytes: 0,
                    dst_y: 0,
                    dst_memory_type: CU_MEMORYTYPE_DEVICE,
                    dst_host: ptr::null_mut(),
                    dst_device: this.buffer,
                    dst_array: ptr::null_mut(),
                    dst_pitch: this.pitch,
                    width_in_bytes: this.width_bytes,
                    height: this.rows,
                };
                r = (api.memcpy_2d)(&p);
                let unmapped = (api.unmap)(1, &mut this.resource, ptr::null_mut());
                api.check(r, "cuMemcpy2D")?;
                api.check(unmapped, "cuGraphicsUnmapResources")
            } else {
                (api.unmap)(1, &mut this.resource, ptr::null_mut());
                api.check(r, "cuGraphicsSubResourceGetMappedArray")
            }
        })
    }

    /// 在本线程推入自己的上下文做事，做完弹出（编码器只在创建它的线程上用）
    fn with_context<T>(&mut self, f: impl FnOnce(&Api, &mut Self) -> Result<T, Error>) -> Result<T, Error> {
        let api = self.api;
        api.check(unsafe { (api.ctx_push)(self.ctx) }, "cuCtxPushCurrent")?;
        let r = f(&api, self);
        let mut popped = ptr::null_mut();
        unsafe { (api.ctx_pop)(&mut popped) };
        r
    }
}

impl Drop for TextureCopy {
    fn drop(&mut self) {
        unsafe {
            if (self.api.ctx_push)(self.ctx) == 0 {
                if self.buffer != 0 {
                    (self.api.mem_free)(self.buffer);
                }
                if !self.resource.is_null() {
                    (self.api.unregister)(self.resource);
                }
                let mut popped = ptr::null_mut();
                (self.api.ctx_pop)(&mut popped);
            }
            (self.api.ctx_destroy)(self.ctx);
        }
    }
}
