// 桌面画面（BGRA 8bit）→ YUV 4:4:4 10bit，三个平面上下叠在一张 R16 纹理里（宽 × 3 倍高）：
// 第 0..h 行是 Y、h..2h 行是 U、2h..3h 行是 V；每个值 16 位、10 位数据放在高位（低 6 位为 0），
// 这是 NVENC 的 NV_ENC_BUFFER_FORMAT_YUV444_10BIT（P1 第 4 项，经 CUDA 拷给 NVENC）。
//
// 矩阵和取整规则同 8bit 着色器（BT.709、全范围），只是按 10 位：Y 用满 0–1023，U / V 以 512 为零点（H.273 全范围）。
// 8bit 源转 10bit 不丢信息，四舍五入即可，不抖动。

Texture2D<float4> src : register(t0);
RWTexture2D<unorm float> dst : register(u0);

cbuffer Params : register(b0)
{
    uint width;
    uint height;
    uint dither;
    uint pad;
};

[numthreads(16, 16, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= width || id.y >= height)
        return;

    float3 rgb = src.Load(int3(id.xy, 0)).rgb;
    float y = dot(rgb, float3(0.2126, 0.7152, 0.0722));
    float cb = (rgb.b - y) / 1.8556;
    float cr = (rgb.r - y) / 1.5748;
    float3 v = clamp(floor(float3(y * 1023.0, cb * 1023.0 + 512.0, cr * 1023.0 + 512.0) + 0.5), 0.0, 1023.0);

    // 10 位值左移 6 位存进 16 位：写 UNORM 时乘 65535 再四舍五入，正好还原成 v × 64
    v = v * 64.0 / 65535.0;
    dst[id.xy] = v.x;
    dst[uint2(id.x, id.y + height)] = v.y;
    dst[uint2(id.x, id.y + 2 * height)] = v.z;
}
