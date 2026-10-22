#ifndef DD_MIC_IOCTL_H
#define DD_MIC_IOCTL_H

/*
 * 虚拟麦克风驱动与 mic-bridge 之间的 IOCTL 约定，两边共用这一份。
 * 控制设备：\\.\DreamDeskMicCtl（只有 SYSTEM / 管理员能开）。
 */

#ifdef _KERNEL_MODE
#include <ntddk.h>
#else
#include <windows.h>
#endif

#define DD_MIC_DEVICE_TYPE 0x8901

/* 0x800 曾是 MAP_BUFFER，从未实现；功能号留空不复用。 */

#define IOCTL_DD_MIC_GET_STATS \
    CTL_CODE(DD_MIC_DEVICE_TYPE, 0x801, METHOD_BUFFERED, FILE_READ_ACCESS)

/* 输入 = 单声道 S16 48 kHz PCM，字节数须为偶数。整块写不下就整块丢（计入溢出）；
 * 没有采集流在跑时失败为 ERROR_NOT_READY。 */
#define IOCTL_DD_MIC_WRITE_PCM \
    CTL_CODE(DD_MIC_DEVICE_TYPE, 0x802, METHOD_BUFFERED, FILE_WRITE_ACCESS)

/* 三个都是驱动加载以来的累计字节数。 */
typedef struct _DD_MIC_STATS {
    LONG underrun_bytes;
    LONG overrun_bytes;
    LONG bytes_consumed_total;
} DD_MIC_STATS, *PDD_MIC_STATS;

#endif /* DD_MIC_IOCTL_H */
