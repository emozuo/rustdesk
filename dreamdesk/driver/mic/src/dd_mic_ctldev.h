/*
 * \\.\DreamDeskMicCtl：mic-bridge 往环里推 PCM 的入口。
 * 单独一个非 PnP 控制设备，因为 PortCls 的音频设备对象不接自定义 IOCTL。
 *
 * 生命周期跟着启动成功的那个音频适配器走，不跟驱动走：驱动自己名下还有设备对象时
 * 系统不会卸载它，控制设备要是活到 DriverUnload，驱动就永远卸不掉，每次升级都得重启。
 */

#ifndef DD_MIC_CTLDEV_H
#define DD_MIC_CTLDEV_H

#include <ntddk.h>

#ifdef __cplusplus
extern "C" {
#endif

/* DriverEntry 里、PcInitializeAdapterDriver 之后调；接管 CREATE / CLOSE /
 * DEVICE_CONTROL，不是发给控制设备的一律经 PcDispatchIrp 还给 PortCls。 */
VOID DdMicControlDeviceHookDispatch(_In_ PDRIVER_OBJECT DriverObject);

/* 适配器 StartDevice 成功后调。已存在则什么都不做。 */
NTSTATUS DdMicControlDeviceCreate(_In_ PDRIVER_OBJECT DriverObject, _In_ PDEVICE_OBJECT OwnerFdo);

/* 适配器 STOP / REMOVE 时调；只有创建它的那个适配器能删（第二个节点起不来，它的 REMOVE 不算）。 */
VOID DdMicControlDeviceDestroy(_In_ PDEVICE_OBJECT OwnerFdo);

#ifdef __cplusplus
}
#endif

#endif /* DD_MIC_CTLDEV_H */
