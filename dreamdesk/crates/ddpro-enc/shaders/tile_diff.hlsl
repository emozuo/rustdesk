// 静止无损（P1 第 7 项）：新采到的画面和上一帧逐像素比，按方块记下「这块变了」。
// 每个线程看一个像素，不一样就把它所在方块的标记置 1（多个线程同时写 1 没有冲突，不用原子操作）；
// 标记缓冲每帧先清零，读回后就是这一帧变了的方块。比的是原始 BGRA 像素，变一个值也算变。

Texture2D<float4> cur : register(t0);
Texture2D<float4> prev : register(t1);
RWStructuredBuffer<uint> changed : register(u0);

cbuffer Params : register(b0)
{
    uint width;
    uint height;
    uint tile;
    uint cols;
};

[numthreads(16, 16, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= width || id.y >= height)
        return;
    if (any(cur.Load(int3(id.xy, 0)) != prev.Load(int3(id.xy, 0))))
        changed[(id.y / tile) * cols + id.x / tile] = 1;
}
