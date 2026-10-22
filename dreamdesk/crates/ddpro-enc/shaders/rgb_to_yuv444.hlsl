// 桌面画面（BGRA 8bit）→ YUV 4:4:4，输出打包的 AYUV：每像素 32 位，从低到高依次是 V、U、Y、A。
// 输出纹理是 DXGI_FORMAT_AYUV，着色器通过 R8G8B8A8_UNORM 视图写它，所以 R=V、G=U、B=Y、A=A。
// （D3D11 下 NVENC 不接受把 Y/U/V 三个平面叠在一张 R8 纹理里的排布，2026-10-13 在 RTX 4060 上
//  实测注册输入时报参数不支持，所以用 AYUV。）
//
// BT.709 矩阵、全范围：Y 用满 0–255，U / V 以 128 为零点；码流里的颜色说明写的也是这两项，
// 解码端按同样的规则还原。
//
// 默认普通四舍五入：8bit 桌面转 8bit YUV，四舍五入就是离原值最近的结果（P1 实测往返最多差 1 级）。
// 有序抖动只在源的精度高于输出时有用，而这个着色器的源永远是 8bit BGRA（HDR 源走 10bit 的着色器）：8bit 源上开它误差反而变大、
// 码流变大，所以正式路径不开，只留给验证工具（capture_encode --dither）对比用。
// 阈值取固定的 8×8 图案而不是随机数，同一画面每帧的结果逐字节相同，「画面没变」的判断不会被抖动破坏。

Texture2D<float4> src : register(t0);
RWTexture2D<unorm float4> dst : register(u0);

cbuffer Params : register(b0)
{
    uint width;
    uint height;
    uint dither;
    uint pad;
};

static const float BAYER8[64] = {
     0, 32,  8, 40,  2, 34, 10, 42,
    48, 16, 56, 24, 50, 18, 58, 26,
    12, 44,  4, 36, 14, 46,  6, 38,
    60, 28, 52, 20, 62, 30, 54, 22,
     3, 35, 11, 43,  1, 33,  9, 41,
    51, 19, 59, 27, 49, 17, 57, 25,
    15, 47,  7, 39, 13, 45,  5, 37,
    63, 31, 55, 23, 61, 29, 53, 21,
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
    float3 v = float3(y * 255.0, cb * 255.0 + 128.0, cr * 255.0 + 128.0);

    float t = dither != 0 ? (BAYER8[(id.y & 7) * 8 + (id.x & 7)] + 0.5) / 64.0 : 0.5;
    v = clamp(floor(v + t), 0.0, 255.0) / 255.0;

    dst[id.xy] = float4(v.z, v.y, v.x, 1.0);
}
