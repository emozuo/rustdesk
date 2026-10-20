//! 显卡上的颜色转换：桌面 BGRA → YUV 4:4:4（打包的 AYUV；着色器见 shaders/rgb_to_yuv444.hlsl）。
//!
//! 采集到的桌面纹理先复制一份到自己的纹理里：采集模块的纹理只在它下一次取帧之前有效，
//! 而且不一定能直接当着色器输入；复制一份后画面没变时也能重编上一帧。

use crate::Error;
use std::ffi::c_void;
use windows::core::Interface;
use windows::Win32::Graphics::{
    Direct3D11::*,
    Dxgi::Common::{
        DXGI_FORMAT, DXGI_FORMAT_AYUV, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
    },
};

static SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rgb_to_yuv444.cso"));
const GROUP: u32 = 16;

pub(crate) struct Converter {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    cs: ID3D11ComputeShader,
    params: ID3D11Buffer,
    source: ID3D11Texture2D,
    source_srv: ID3D11ShaderResourceView,
    output: ID3D11Texture2D,
    output_uav: ID3D11UnorderedAccessView,
    width: u32,
    height: u32,
}

impl Converter {
    pub fn new(device: &ID3D11Device, width: u32, height: u32, dither: bool) -> Result<Self, Error> {
        unsafe {
            let ctx = device.GetImmediateContext()?;

            let mut cs = None;
            device.CreateComputeShader(SHADER, None, Some(&mut cs))?;

            let params: [u32; 4] = [width, height, dither as u32, 0];
            // 抖动运行中能切（精确模式），参数缓冲要可更新
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of_val(&params) as u32,
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

            let source = texture(device, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_BIND_SHADER_RESOURCE.0 as u32)?;
            let mut source_srv = None;
            device.CreateShaderResourceView(&source, None, Some(&mut source_srv))?;

            // 打包的 AYUV；着色器通过 R8G8B8A8_UNORM 视图写它（D3D11 允许的 AYUV 视图格式）
            let output = texture(
                device,
                width,
                height,
                DXGI_FORMAT_AYUV,
                (D3D11_BIND_UNORDERED_ACCESS.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            )?;
            let uav_desc = D3D11_UNORDERED_ACCESS_VIEW_DESC {
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_UAV { MipSlice: 0 },
                },
            };
            let mut output_uav = None;
            device.CreateUnorderedAccessView(&output, Some(&uav_desc), Some(&mut output_uav))?;

            Ok(Self {
                device: device.clone(),
                ctx,
                cs: cs.ok_or_else(|| Error::Config("CreateComputeShader returned nothing".to_owned()))?,
                params: buf.ok_or_else(|| Error::Config("CreateBuffer returned nothing".to_owned()))?,
                source,
                source_srv: source_srv.ok_or_else(|| Error::Config("CreateShaderResourceView returned nothing".to_owned()))?,
                output,
                output_uav: output_uav.ok_or_else(|| Error::Config("CreateUnorderedAccessView returned nothing".to_owned()))?,
                width,
                height,
            })
        }
    }

    /// 开 / 关抖动，下一次转换生效。
    pub fn set_dither(&self, dither: bool) {
        let params: [u32; 4] = [self.width, self.height, dither as u32, 0];
        unsafe {
            self.ctx
                .UpdateSubresource(&self.params, 0, None, params.as_ptr() as *const c_void, 0, 0);
        }
    }

    /// NVENC 注册用的输出纹理。
    pub fn output_ptr(&self) -> *mut c_void {
        self.output.as_raw()
    }

    /// 载入一帧采集到的桌面纹理。尺寸或格式不对就报错：分辨率变了要重建编码器，
    /// HDR 等非 8bit 桌面在 P0 不支持，不悄悄转换。
    pub fn load_source(&mut self, src: *mut c_void) -> Result<(), Error> {
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
            self.ctx.CopyResource(&self.source, src);
        }
        Ok(())
    }

    pub fn convert(&self) {
        unsafe {
            self.ctx.CSSetShader(&self.cs, None);
            self.ctx.CSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            self.ctx.CSSetShaderResources(0, Some(&[Some(self.source_srv.clone())]));
            self.ctx.CSSetUnorderedAccessViews(0, 1, Some(&Some(self.output_uav.clone())), None);
            self.ctx.Dispatch(self.width.div_ceil(GROUP), self.height.div_ceil(GROUP), 1);
            // 解绑，NVENC 读输出纹理前它不能还挂在着色器上
            self.ctx.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
            self.ctx.CSSetShaderResources(0, Some(&[None]));
        }
    }

    pub fn read_source_bgra(&self) -> Result<Vec<u8>, Error> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            self.source.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.ok_or_else(|| Error::Config("CreateTexture2D returned nothing".to_owned()))?;
            self.ctx.CopyResource(&staging, &self.source);
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
