//! 显卡上的颜色转换：桌面 BGRA → YUV 4:4:4（打包的 AYUV；着色器见 shaders/rgb_to_yuv444.hlsl）。
//!
//! 采集到的桌面纹理先复制一份到自己的纹理里：采集模块的纹理只在它下一次取帧之前有效，
//! 而且不一定能直接当着色器输入；复制一份后画面没变时也能重编上一帧。
//!
//! 静止无损（P1 第 7 项）：自己的源纹理有两份轮流用，新画面复制进另一份后和上一帧按方块逐像素比
//! （shaders/tile_diff.hlsl），得出「这一帧哪些方块变了」；静止的方块按需从当前这份读回原始像素。

use crate::Error;
use std::ffi::c_void;
use windows::core::Interface;
use windows::Win32::Graphics::{
    Direct3D11::*,
    Dxgi::Common::{
        DXGI_FORMAT, DXGI_FORMAT_AYUV, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
    },
};

static SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rgb_to_yuv444.cso"));
/// 10bit：三个平面叠在一张 R16 纹理里（shaders/rgb_to_yuv444_10.hlsl，第 4 项）
static SHADER_10: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rgb_to_yuv444_10.cso"));
static DIFF_SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tile_diff.cso"));
const GROUP: u32 = 16;

/// 按方块比两帧要的东西
struct Diff {
    cs: ID3D11ComputeShader,
    params: ID3D11Buffer,
    flags: ID3D11Buffer,
    flags_uav: ID3D11UnorderedAccessView,
    /// 标记读回用（CPU 可读）
    flags_read: ID3D11Buffer,
    tiles: usize,
}

pub(crate) struct Converter {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    cs: ID3D11ComputeShader,
    params: ID3D11Buffer,
    /// 两份源纹理轮流用：`cur` 是当前这一帧，另一份是上一帧（下一帧复制进它）
    sources: [ID3D11Texture2D; 2],
    source_srvs: [ID3D11ShaderResourceView; 2],
    cur: usize,
    /// 已经载入过画面（有上一帧可比）
    loaded: bool,
    output: ID3D11Texture2D,
    output_uav: ID3D11UnorderedAccessView,
    diff: Option<Diff>,
    /// 读回方块用的暂存纹理（CPU 可读，第一次读时建）
    readback: Option<ID3D11Texture2D>,
    width: u32,
    height: u32,
}

impl Converter {
    /// `tile` 是按方块比两帧用的方块边长，0 = 不比。`ten_bit` 时输出 10bit 三平面（给 CUDA 拷走），否则打包的 8bit AYUV。
    pub fn new(device: &ID3D11Device, width: u32, height: u32, dither: bool, tile: u32, ten_bit: bool) -> Result<Self, Error> {
        unsafe {
            let ctx = device.GetImmediateContext()?;

            let mut cs = None;
            device.CreateComputeShader(if ten_bit { SHADER_10 } else { SHADER }, None, Some(&mut cs))?;

            let params: [u32; 4] = [width, height, dither as u32, 0];
            // 抖动运行中能切（主控端「设置抖动」），参数缓冲要可更新
            let buf = constant_buffer(device, &params)?;

            let source = |_| -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView), Error> {
                let t = texture(device, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_BIND_SHADER_RESOURCE.0 as u32)?;
                let mut srv = None;
                device.CreateShaderResourceView(&t, None, Some(&mut srv))?;
                Ok((t, srv.ok_or_else(|| Error::Config("CreateShaderResourceView returned nothing".to_owned()))?))
            };
            let (s0, v0) = source(0)?;
            let (s1, v1) = source(1)?;

            // 8bit：打包的 AYUV，着色器通过 R8G8B8A8_UNORM 视图写它（D3D11 允许的 AYUV 视图格式）。
            // 10bit：R16 纹理、3 倍高，Y / U / V 上下叠放；CUDA 映射它要能共享，不用 RENDER_TARGET
            let (out_format, out_height, uav_format, bind) = if ten_bit {
                (DXGI_FORMAT_R16_UNORM, height * 3, DXGI_FORMAT_R16_UNORM, D3D11_BIND_UNORDERED_ACCESS.0 as u32)
            } else {
                (
                    DXGI_FORMAT_AYUV,
                    height,
                    DXGI_FORMAT_R8G8B8A8_UNORM,
                    (D3D11_BIND_UNORDERED_ACCESS.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                )
            };
            let output = texture(device, width, out_height, out_format, bind)?;
            let uav_desc = D3D11_UNORDERED_ACCESS_VIEW_DESC {
                Format: uav_format,
                ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_UAV { MipSlice: 0 },
                },
            };
            let mut output_uav = None;
            device.CreateUnorderedAccessView(&output, Some(&uav_desc), Some(&mut output_uav))?;

            let diff = if tile > 0 { Some(Diff::new(device, width, height, tile)?) } else { None };

            Ok(Self {
                device: device.clone(),
                ctx,
                cs: cs.ok_or_else(|| Error::Config("CreateComputeShader returned nothing".to_owned()))?,
                params: buf,
                sources: [s0, s1],
                source_srvs: [v0, v1],
                cur: 0,
                loaded: false,
                output,
                output_uav: output_uav.ok_or_else(|| Error::Config("CreateUnorderedAccessView returned nothing".to_owned()))?,
                diff,
                readback: None,
                width,
                height,
            })
        }
    }

    /// NVENC 注册用的输出纹理。
    pub fn output_ptr(&self) -> *mut c_void {
        self.output.as_raw()
    }

    /// 输出纹理（10bit 时交给 CUDA 每帧拷走）
    pub fn output(&self) -> &ID3D11Texture2D {
        &self.output
    }

    /// 载入一帧采集到的桌面纹理。尺寸或格式不对就报错：分辨率变了要重建编码器，
    /// HDR 等非 8bit 桌面在 P0 不支持，不悄悄转换。
    /// 按方块比（建时给了方块边长）时返回这一帧哪些方块变了（每位一块、低位在前）；第一帧没得比，算全变了。
    pub fn load_source(&mut self, src: *mut c_void) -> Result<Option<Vec<u8>>, Error> {
        unsafe {
            let src = ID3D11Texture2D::from_raw_borrowed(&src)
                .ok_or_else(|| Error::Config("null source texture".to_owned()))?;
            let mut d = D3D11_TEXTURE2D_DESC::default();
            src.GetDesc(&mut d);
            if d.Width != self.width || d.Height != self.height {
                return Err(Error::Config(format!(
                    "source is {}x{}, encoder is {}x{}: recreate the encoder",
                    d.Width, d.Height, self.width, self.height
                )));
            }
            if d.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                return Err(Error::Unsupported(format!("desktop format {:?} is not 8-bit BGRA", d.Format)));
            }
            let next = 1 - self.cur;
            self.ctx.CopyResource(&self.sources[next], src);
            let changed = match &self.diff {
                Some(diff) if self.loaded => Some(diff.run(&self.ctx, &self.source_srvs[next], &self.source_srvs[self.cur], self.width, self.height)?),
                Some(diff) => Some(vec![0xff; diff.tiles.div_ceil(8)]),
                None => None,
            };
            self.cur = next;
            self.loaded = true;
            Ok(changed)
        }
    }

    /// 画面没变（采集说没有新帧）：按方块比时全是 0。
    pub fn unchanged(&self) -> Option<Vec<u8>> {
        self.diff.as_ref().map(|d| vec![0; d.tiles.div_ceil(8)])
    }

    pub fn convert(&self) {
        unsafe {
            self.ctx.CSSetShader(&self.cs, None);
            self.ctx.CSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            self.ctx.CSSetShaderResources(0, Some(&[Some(self.source_srvs[self.cur].clone())]));
            self.ctx.CSSetUnorderedAccessViews(0, 1, Some(&Some(self.output_uav.clone())), None);
            self.ctx.Dispatch(self.width.div_ceil(GROUP), self.height.div_ceil(GROUP), 1);
            // 解绑，NVENC 读输出纹理前它不能还挂在着色器上
            self.ctx.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
            self.ctx.CSSetShaderResources(0, Some(&[None]));
        }
    }

    /// 从当前这一帧读回几块原始像素：`rects` 是 (x, y, 宽, 高)，返回每块的 BGRA（逐行紧排）。
    /// 一次映射读完：先把各块拷进暂存纹理同样的位置，再映射一次。
    pub fn read_tiles(&mut self, rects: &[(u32, u32, u32, u32)]) -> Result<Vec<Vec<u8>>, Error> {
        unsafe {
            if self.readback.is_none() {
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                self.sources[self.cur].GetDesc(&mut desc);
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                let mut t = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut t))?;
                self.readback = t;
            }
            let staging = self
                .readback
                .as_ref()
                .ok_or_else(|| Error::Config("CreateTexture2D returned nothing".to_owned()))?;
            for &(x, y, w, h) in rects {
                let b = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
                self.ctx.CopySubresourceRegion(staging, 0, x, y, 0, &self.sources[self.cur], 0, Some(&b));
            }
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let out = rects
                .iter()
                .map(|&(x, y, w, h)| {
                    let mut px = Vec::with_capacity((w * h * 4) as usize);
                    for row in y..y + h {
                        let p = (m.pData as *const u8).add(row as usize * m.RowPitch as usize + x as usize * 4);
                        px.extend_from_slice(std::slice::from_raw_parts(p, w as usize * 4));
                    }
                    px
                })
                .collect();
            self.ctx.Unmap(staging, 0);
            Ok(out)
        }
    }

    pub fn read_source_bgra(&self) -> Result<Vec<u8>, Error> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            self.sources[self.cur].GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.ok_or_else(|| Error::Config("CreateTexture2D returned nothing".to_owned()))?;
            self.ctx.CopyResource(&staging, &self.sources[self.cur]);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = self.width as usize * 4;
            let mut out = Vec::with_capacity(row * self.height as usize);
            for y in 0..self.height as usize {
                let p = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                out.extend_from_slice(std::slice::from_raw_parts(p, row));
            }
            self.ctx.Unmap(&staging, 0);
            Ok(out)
        }
    }
}

impl Diff {
    unsafe fn new(device: &ID3D11Device, width: u32, height: u32, tile: u32) -> Result<Self, Error> {
        let mut cs = None;
        device.CreateComputeShader(DIFF_SHADER, None, Some(&mut cs))?;
        let cols = width.div_ceil(tile);
        let tiles = (cols * height.div_ceil(tile)) as usize;
        let params = constant_buffer(device, &[width, height, tile, cols])?;
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: (tiles * 4) as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
            StructureByteStride: 4,
            ..Default::default()
        };
        let mut flags = None;
        device.CreateBuffer(&desc, None, Some(&mut flags))?;
        let flags = flags.ok_or_else(|| Error::Config("CreateBuffer returned nothing".to_owned()))?;
        let uav_desc = D3D11_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_UNKNOWN,
            ViewDimension: D3D11_UAV_DIMENSION_BUFFER,
            Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D11_BUFFER_UAV {
                    FirstElement: 0,
                    NumElements: tiles as u32,
                    Flags: 0,
                },
            },
        };
        let mut flags_uav = None;
        device.CreateUnorderedAccessView(&flags, Some(&uav_desc), Some(&mut flags_uav))?;
        let read_desc = D3D11_BUFFER_DESC {
            ByteWidth: (tiles * 4) as u32,
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..Default::default()
        };
        let mut flags_read = None;
        device.CreateBuffer(&read_desc, None, Some(&mut flags_read))?;
        Ok(Self {
            cs: cs.ok_or_else(|| Error::Config("CreateComputeShader returned nothing".to_owned()))?,
            params,
            flags,
            flags_uav: flags_uav.ok_or_else(|| Error::Config("CreateUnorderedAccessView returned nothing".to_owned()))?,
            flags_read: flags_read.ok_or_else(|| Error::Config("CreateBuffer returned nothing".to_owned()))?,
            tiles,
        })
    }

    /// 比 `cur` 和 `prev`，读回每块的标记，打成位图（每位一块、低位在前）。读回要等显卡比完（209.32 RTX 4060 实测整屏 0.08 毫秒）
    unsafe fn run(
        &self,
        ctx: &ID3D11DeviceContext,
        cur: &ID3D11ShaderResourceView,
        prev: &ID3D11ShaderResourceView,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, Error> {
        ctx.ClearUnorderedAccessViewUint(&self.flags_uav, &[0; 4]);
        ctx.CSSetShader(&self.cs, None);
        ctx.CSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
        ctx.CSSetShaderResources(0, Some(&[Some(cur.clone()), Some(prev.clone())]));
        ctx.CSSetUnorderedAccessViews(0, 1, Some(&Some(self.flags_uav.clone())), None);
        ctx.Dispatch(width.div_ceil(GROUP), height.div_ceil(GROUP), 1);
        ctx.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
        ctx.CSSetShaderResources(0, Some(&[None, None]));
        ctx.CopyResource(&self.flags_read, &self.flags);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&self.flags_read, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let f = std::slice::from_raw_parts(m.pData as *const u32, self.tiles);
        let mut bitmap = vec![0u8; self.tiles.div_ceil(8)];
        for (t, &v) in f.iter().enumerate() {
            if v != 0 {
                bitmap[t / 8] |= 1 << (t % 8);
            }
        }
        ctx.Unmap(&self.flags_read, 0);
        Ok(bitmap)
    }
}

unsafe fn constant_buffer(device: &ID3D11Device, params: &[u32; 4]) -> Result<ID3D11Buffer, Error> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: std::mem::size_of_val(params) as u32,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        ..Default::default()
    };
    let init = D3D11_SUBRESOURCE_DATA {
        pSysMem: params.as_ptr() as *const c_void,
        ..Default::default()
    };
    let mut buf = None;
    device.CreateBuffer(&desc, Some(&init), Some(&mut buf))?;
    buf.ok_or_else(|| Error::Config("CreateBuffer returned nothing".to_owned()))
}

unsafe fn texture(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, bind: u32) -> Result<ID3D11Texture2D, Error> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        ..Default::default()
    };
    let mut t = None;
    device.CreateTexture2D(&desc, None, Some(&mut t))?;
    t.ok_or_else(|| Error::Config("CreateTexture2D returned nothing".to_owned()))
}
