// HDR 桌面（scRGB：R16G16B16A16_FLOAT，线性、BT.709 原色、1.0 = 80 尼特）→ BT.2020 + PQ 的 YUV 4:4:4 10bit（HDR10 的颜色规则），
// 输出排布同 rgb_to_yuv444_10.hlsl：Y/U/V 叠在一张 R16 纹理里（宽 × 3 倍高），10 位放高位。P1 第 4 项。
//
// 步骤：× 80 换成尼特 → BT.709 原色转 BT.2020 原色（线性下做，ITU-R BT.2087 的矩阵）→ 负值截到 0（scRGB 能表示 BT.709 以外的颜色，
// 落在 BT.2020 里的保留下来，BT.2020 也装不下的截掉）→ PQ 编码（SMPTE ST 2084，10000 尼特为满）→ BT.2020 非恒定亮度矩阵 → 10 位全范围。
// 码流里的颜色说明写 BT.2020 / SMPTE 2084 / BT.2020 NCL、全范围，主控端按同样的规则还原。

Texture2D<float4> src : register(t0);
RWTexture2D<unorm float> dst : register(u0);

cbuffer Params : register(b0)
{
    uint width;
    uint height;
    uint dither;
    uint pad;
};

static const float3x3 BT709_TO_BT2020 = {
    0.6274040, 0.3292820, 0.0433136,
    0.0690970, 0.9195400, 0.0113612,
    0.0163916, 0.0880132, 0.8955950,
};

float3 pq_encode(float3 nits)
{
    float3 y = pow(saturate(nits / 10000.0), 0.1593017578125);
    return pow((0.8359375 + 18.8515625 * y) / (1.0 + 18.6875 * y), 78.84375);
}

[numthreads(16, 16, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= width || id.y >= height)
        return;

    float3 lin709 = src.Load(int3(id.xy, 0)).rgb;
    float3 nits = max(mul(BT709_TO_BT2020, lin709 * 80.0), 0.0);
    float3 rgb = pq_encode(nits);
    float y = dot(rgb, float3(0.2627, 0.6780, 0.0593));
    float cb = (rgb.b - y) / 1.8814;
    float cr = (rgb.r - y) / 1.4746;
    float3 v = clamp(floor(float3(y * 1023.0, cb * 1023.0 + 512.0, cr * 1023.0 + 512.0) + 0.5), 0.0, 1023.0);

    v = v * 64.0 / 65535.0;
    dst[id.xy] = v.x;
    dst[uint2(id.x, id.y + height)] = v.y;
    dst[uint2(id.x, id.y + 2 * height)] = v.z;
}
