#include "dd_mic_ring.h"

/* 16 KB ≈ 170 ms 单声道 S16 48 kHz；主控端一包 5 ms（480 字节）。门限之上留足余量接住
 * 晚到的包随后一起涌来的突发。静态存储：驱动映像的 .data 非分页，DPC 里可以直接碰。 */
#define RING_BYTES 16384u
#define RING_MASK  (RING_BYTES - 1u)
C_ASSERT((RING_BYTES & RING_MASK) == 0);

/* 起播门限（抖动缓冲）：攒够 40 ms 后的**下一次**读才开始出数，取空后重新攒。
 * 消费者一次读的是上次读以来流逝的时长，实测单次最多 19 ms；要是攒够的那一次就开始读，
 * 头一口大读把余量吃到只剩几毫秒（09-23 .68 实测）。原来是 20 ms，网络偶尔晚到 20 ms 以上
 * 就取空断一下（10-14 实测几分钟一次）。代价是麦克风延迟 40 ms，说话察觉不到。 */
#define PRIME_BYTES 3840u

/* 高水位：晚到的包随后一起涌来，水位就停在高处——没有时钟回收，延迟只涨不跌。
 * 超过 100 ms 就丢到门限，丢掉的计入溢出。 */
#define HIGH_BYTES 9600u

/* 取空的断口、恢复的起点、高水位丢数据的接缝各做 2 ms 渐变：信号直接跳变是一个台阶，听起来就是「哒」。 */
#define FADE_SAMPLES 96

static struct {
    volatile LONG write_idx;   /* 只有生产者推进 */
    volatile LONG read_idx;    /* 只有消费者推进；生产者写不下时丢新的，绝不碰它 */
    /* 从 primed 到 splice_pos 只有消费者读写（开录前由 DiscardAndActivate 复位）。 */
    BOOLEAN       primed;
    SHORT         last_sample; /* 最后交出去的那个真实样本 */
    SHORT         fade_from;   /* 断口渐出的起点值；fade_out 是已渐出的样本数，可跨多次读 */
    LONG          fade_out;
    LONG          fade_in;     /* 恢复出数后还要渐入的样本数 */
    /* 高水位丢数据的接缝：旧位置开头 FADE 个样本拷在这里，交叉渐变按 splice_pos 跨多次读走完
     * （引擎按整毫秒推进时一次只读 48 个样本，只在单次读里做会被截断）。 */
    SHORT         splice_old[FADE_SAMPLES];
    LONG          splice_pos;  /* == FADE_SAMPLES 表示没有进行中的接缝 */
    volatile LONG active;
    volatile LONG underrun_bytes;
    volatile LONG overrun_bytes;
    volatile LONG bytes_consumed_total;
    UCHAR         data[RING_BYTES];
} g_ring;

/* 硬件音量：属性线程写 db / mute / target，消费者把 cur 逐样本趋近 target（Q15，32768 = 0 dB）。 */
static volatile LONG g_level_db;
static volatile LONG g_level_mute;
static volatile LONG g_level_target = 32768;
static LONG          g_level_cur = 32768;

static VOID level_update_target(VOID)
{
    LONG steps = -InterlockedCompareExchange(&g_level_db, 0, 0) / DD_MIC_VOLUME_STEP;
    LONGLONG g = 1LL << 30;
    /* 10^(-0.5/20) 的 Q30。属性请求在 PASSIVE 级，但不碰浮点省得存 FPU 状态。 */
    for (LONG i = 0; i < steps; ++i) g = (g * 1013677647LL) >> 30;
    InterlockedExchange(&g_level_target,
                        InterlockedCompareExchange(&g_level_mute, 0, 0) ? 0 : (LONG)(g >> 15));
}

VOID DdMicRingSetVolume(_In_ LONG db_16_16)
{
    if (db_16_16 < DD_MIC_VOLUME_MIN) db_16_16 = DD_MIC_VOLUME_MIN;
    if (db_16_16 > DD_MIC_VOLUME_MAX) db_16_16 = DD_MIC_VOLUME_MAX;
    db_16_16 = (db_16_16 - DD_MIC_VOLUME_STEP / 2) / DD_MIC_VOLUME_STEP * DD_MIC_VOLUME_STEP;
    InterlockedExchange(&g_level_db, db_16_16);
    level_update_target();
}

LONG DdMicRingGetVolume(VOID)
{
    return InterlockedCompareExchange(&g_level_db, 0, 0);
}

VOID DdMicRingSetMute(_In_ BOOLEAN mute)
{
    InterlockedExchange(&g_level_mute, mute ? 1 : 0);
    level_update_target();
}

BOOLEAN DdMicRingGetMute(VOID)
{
    return InterlockedCompareExchange(&g_level_mute, 0, 0) != 0;
}

/* 每样本向目标走 1/64（时间常数约 1.3 ms），换音量不出拉链声。 */
static VOID level_apply(_Inout_updates_(n) SHORT *s, _In_ LONG n)
{
    LONG target = InterlockedCompareExchange(&g_level_target, 0, 0);
    LONG cur = g_level_cur;
    if (cur == 32768 && target == 32768) return;
    for (LONG i = 0; i < n; ++i) {
        LONG d = target - cur;
        cur = (d > -64 && d < 64) ? target : cur + d / 64;
        s[i] = (SHORT)(((LONG)s[i] * cur) >> 15);
    }
    g_level_cur = cur;
}

BOOLEAN DdMicRingIsActive(VOID)
{
    return InterlockedCompareExchange(&g_ring.active, 0, 0) != 0;
}

VOID DdMicRingSetActive(_In_ BOOLEAN active)
{
    InterlockedExchange(&g_ring.active, active ? 1 : 0);
}

VOID DdMicRingDiscardAndActivate(VOID)
{
    InterlockedExchange(&g_ring.read_idx, InterlockedCompareExchange(&g_ring.write_idx, 0, 0));
    g_ring.primed = FALSE;
    g_ring.last_sample = 0;
    g_ring.fade_out = FADE_SAMPLES;
    g_ring.fade_in = 0;
    g_ring.splice_pos = FADE_SAMPLES;
    g_level_cur = InterlockedCompareExchange(&g_level_target, 0, 0);
    DdMicRingSetActive(TRUE);
}

ULONG DdMicRingWrite(_In_reads_bytes_(bytes) const VOID *src, _In_ ULONG bytes)
{
    if (!src || bytes == 0) return 0;

    LONG w = InterlockedCompareExchange(&g_ring.write_idx, 0, 0);
    LONG r = InterlockedCompareExchange(&g_ring.read_idx, 0, 0);
    ULONG used = (ULONG)(w - r) & RING_MASK;
    ULONG free_bytes = RING_MASK - used;

    /* 整块丢而不是截断：截成奇数字节会让之后每个 S16 样本错位半个。 */
    if (bytes > free_bytes) {
        InterlockedAdd(&g_ring.overrun_bytes, (LONG)bytes);
        return 0;
    }

    const UCHAR *s = (const UCHAR *)src;
    ULONG woff = (ULONG)w & RING_MASK;
    ULONG first = RING_BYTES - woff;
    if (first > bytes) first = bytes;
    RtlCopyMemory(&g_ring.data[woff], s, first);
    if (bytes > first) RtlCopyMemory(g_ring.data, s + first, bytes - first);

    InterlockedExchange(&g_ring.write_idx, (LONG)(((ULONG)w + bytes) & RING_MASK));
    return bytes;
}

VOID DdMicRingRead(_Out_writes_bytes_(bytes) PVOID dst, _In_ ULONG bytes)
{
    LONG w = InterlockedCompareExchange(&g_ring.write_idx, 0, 0);
    LONG r = InterlockedCompareExchange(&g_ring.read_idx, 0, 0);
    ULONG used = (ULONG)(w - r) & RING_MASK;
    UCHAR *d = (UCHAR *)dst;

    if (g_ring.primed && used > HIGH_BYTES) {
        ULONG drop = (used - PRIME_BYTES) & ~1u;
        /* 旧数据还在环里，必须在发布新的 read_idx 之前拷走，之后生产者就可能覆盖它。 */
        for (LONG i = 0; i < FADE_SAMPLES; ++i)
            g_ring.splice_old[i] = *(SHORT *)&g_ring.data[((ULONG)r + 2u * (ULONG)i) & RING_MASK];
        g_ring.splice_pos = 0;
        r = (LONG)(((ULONG)r + drop) & RING_MASK);
        used -= drop;
        InterlockedAdd(&g_ring.overrun_bytes, (LONG)drop);
    }
    ULONG to_read = !g_ring.primed ? 0 : (used < bytes ? used : bytes);

    if (to_read > 0) {
        ULONG roff = (ULONG)r & RING_MASK;
        ULONG first = RING_BYTES - roff;
        if (first > to_read) first = to_read;
        RtlCopyMemory(d, &g_ring.data[roff], first);
        if (to_read > first) RtlCopyMemory(d + first, g_ring.data, to_read - first);

        /* 整数运算：DISPATCH_LEVEL 下碰浮点要先存 FPU 状态。 */
        SHORT *s = (SHORT *)d;
        LONG n = (LONG)(to_read / sizeof(SHORT));
        /* 丢数据的接缝：从「本该接着播的旧数据」交叉渐变到新位置。 */
        for (LONG i = 0; i < n && g_ring.splice_pos < FADE_SAMPLES; ++i, ++g_ring.splice_pos) {
            LONG p = g_ring.splice_pos;
            s[i] = (SHORT)(((LONG)g_ring.splice_old[p] * (FADE_SAMPLES - p) + (LONG)s[i] * p) / FADE_SAMPLES);
        }
        InterlockedExchange(&g_ring.read_idx, (LONG)(((ULONG)r + to_read) & RING_MASK));
        InterlockedAdd(&g_ring.bytes_consumed_total, (LONG)to_read);

        for (LONG i = 0; i < n && g_ring.fade_in > 0; ++i, --g_ring.fade_in)
            s[i] = (SHORT)((LONG)s[i] * (FADE_SAMPLES - g_ring.fade_in) / FADE_SAMPLES);
        g_ring.last_sample = s[n - 1];
        g_ring.fade_out = 0;   /* 有真实数据交出去了，下一个断口重新从它渐出 */
    }

    /* 补零的字节都计入取空，含攒门限期间的：否则生产者整个没来时这个数一直是 0。 */
    if (to_read < bytes) {
        RtlZeroMemory(d + to_read, bytes - to_read);
        if (g_ring.fade_out == 0) g_ring.fade_from = g_ring.last_sample;
        SHORT *z = (SHORT *)(d + to_read);
        LONG zn = (LONG)((bytes - to_read) / sizeof(SHORT));
        for (LONG i = 0; i < zn && g_ring.fade_out < FADE_SAMPLES; ++i, ++g_ring.fade_out)
            z[i] = (SHORT)((LONG)g_ring.fade_from * (FADE_SAMPLES - 1 - g_ring.fade_out) / FADE_SAMPLES);
        InterlockedAdd(&g_ring.underrun_bytes, (LONG)(bytes - to_read));
        g_ring.splice_pos = FADE_SAMPLES;   /* 接缝没走完就断了：恢复后接的是新数据，不再渐变 */
        BOOLEAN was_primed = g_ring.primed;
        /* 攒门限期间：够了就下一次开始读。渐出没走完不起播，否则渐入从 0 开始又是一个台阶。 */
        g_ring.primed = used >= PRIME_BYTES && g_ring.fade_out >= FADE_SAMPLES;
        if (g_ring.primed && !was_primed) g_ring.fade_in = FADE_SAMPLES;
    }
    level_apply((SHORT *)d, (LONG)(bytes / sizeof(SHORT)));
}

VOID DdMicRingGetStats(_Out_ PDD_MIC_STATS stats)
{
    stats->underrun_bytes       = InterlockedCompareExchange(&g_ring.underrun_bytes, 0, 0);
    stats->overrun_bytes        = InterlockedCompareExchange(&g_ring.overrun_bytes, 0, 0);
    stats->bytes_consumed_total = InterlockedCompareExchange(&g_ring.bytes_consumed_total, 0, 0);
}
