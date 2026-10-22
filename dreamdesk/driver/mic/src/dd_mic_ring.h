/*
 * 虚拟麦克风的 PCM 环：全驱动唯一一份，单生产者单消费者、无锁。
 * 生产者 = IOCTL_DD_MIC_WRITE_PCM 的派发线程；消费者 = 采集流的 WriteBytes
 * （流的 m_PositionSpinLock 下，DISPATCH_LEVEL）。字节格式是单声道 S16 48 kHz。
 */

#ifndef DD_MIC_RING_H
#define DD_MIC_RING_H

#include <ntddk.h>
#include "../include/dd_mic_ioctl.h"

#ifdef __cplusplus
extern "C" {
#endif

/* 有采集流在 RUN 才收数据；否则 WRITE_PCM 回 STATUS_DEVICE_NOT_READY，
 * 免得没人录音时 mic-bridge 把环灌满、溢出计数一直涨。 */
BOOLEAN DdMicRingIsActive(VOID);
VOID    DdMicRingSetActive(_In_ BOOLEAN active);

/* 流进 RUN 时由消费者调用：丢掉环里的旧数据再开闸，否则开录头几十毫秒
 * 是上一次停录前的残留。只能在消费者没在跑的时候调（它推进 read_idx）。 */
VOID DdMicRingDiscardAndActivate(VOID);

/* 写得下就整块写入并返回 bytes；写不下一个字节都不写、返回 0，计入溢出。 */
ULONG DdMicRingWrite(_In_reads_bytes_(bytes) const VOID *src, _In_ ULONG bytes);

/* 攒够起播门限之后的下一次读才出数；之前、以及读不够时补零并计入取空，取空后重新攒。
 * 断口渐出到 0、恢复处渐入，不留台阶。 */
VOID DdMicRingRead(_Out_writes_bytes_(bytes) PVOID dst, _In_ ULONG bytes);

VOID DdMicRingGetStats(_Out_ PDD_MIC_STATS stats);

/* 麦克风端点的硬件音量（拓扑 VOLUME / MUTE 节点），读出时作用到样本上，换音量有渐变。
 * 必须是硬件音量：交给引擎做软件音量的话范围是 -96..+30 dB，滑块 100% = +30 dB，
 * 叠在 mic-bridge 的 +18 dB 上直接过载（实测）。dB 为 16.16 定点，夹到 [MIN, MAX]、按 STEP 取整。 */
#define DD_MIC_VOLUME_MIN  (-96 * 0x10000)
#define DD_MIC_VOLUME_MAX  0
#define DD_MIC_VOLUME_STEP 0x8000          /* 0.5 dB */
VOID    DdMicRingSetVolume(_In_ LONG db_16_16);
LONG    DdMicRingGetVolume(VOID);
VOID    DdMicRingSetMute(_In_ BOOLEAN mute);
BOOLEAN DdMicRingGetMute(VOID);

#ifdef __cplusplus
}
#endif

#endif /* DD_MIC_RING_H */
