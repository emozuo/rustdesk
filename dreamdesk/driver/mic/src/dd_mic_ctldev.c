#include "dd_mic_ctldev.h"
#include "dd_mic_ring.h"
#include <wdmsec.h>
/* portcls.h 按 C++ 写的，C 里包含会报 C4115（括号里的具名类型定义）；只用它的 PcDispatchIrp。 */
#pragma warning(push)
#pragma warning(disable: 4115)
#include <portcls.h>
#pragma warning(pop)

/* 只在 PnP 回调里改（PnP 管理器串行）。派发按设备类型认控制设备，不靠这个指针，
 * 所以删除之后还开着的旧句柄照样安全走到这里；和删除赛跑最多让一发写进静态的环。 */
static PDEVICE_OBJECT g_CtlDeviceObject = NULL;
static PDEVICE_OBJECT g_CtlOwnerFdo = NULL;

static NTSTATUS CompleteIrp(PIRP Irp, NTSTATUS status, ULONG_PTR information)
{
    Irp->IoStatus.Status = status;
    Irp->IoStatus.Information = information;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return status;
}

/* PortCls 的 FDO 是 FILE_DEVICE_KS；控制设备用我们自己的类型，和 IOCTL 里的一致。 */
static BOOLEAN IsControlDevice(PDEVICE_OBJECT DeviceObject)
{
    return DeviceObject->DeviceType == DD_MIC_DEVICE_TYPE;
}

static NTSTATUS DdMicCtlIrpCreateClose(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    if (!IsControlDevice(DeviceObject)) return PcDispatchIrp(DeviceObject, Irp);
    return CompleteIrp(Irp, STATUS_SUCCESS, 0);
}

/* PortCls 替其余几个 major 装的处理函数假定设备扩展是它的；控制设备的这类 IRP
 * （比如工具查句柄的安全描述符）不能漏过去。 */
static NTSTATUS DdMicCtlIrpOther(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    if (!IsControlDevice(DeviceObject)) return PcDispatchIrp(DeviceObject, Irp);
    return CompleteIrp(Irp, STATUS_INVALID_DEVICE_REQUEST, 0);
}

static NTSTATUS DdMicCtlIrpDeviceControl(PDEVICE_OBJECT DeviceObject, PIRP Irp)
{
    if (!IsControlDevice(DeviceObject)) return PcDispatchIrp(DeviceObject, Irp);

    PIO_STACK_LOCATION ioStack = IoGetCurrentIrpStackLocation(Irp);
    PVOID buf = Irp->AssociatedIrp.SystemBuffer;

    /* 适配器已移除、这是删除前开的旧句柄：让用户态关掉重开，否则它攥着句柄，驱动卸不掉。 */
    if (DeviceObject != g_CtlDeviceObject) return CompleteIrp(Irp, STATUS_DEVICE_REMOVED, 0);

    switch (ioStack->Parameters.DeviceIoControl.IoControlCode) {
    case IOCTL_DD_MIC_WRITE_PCM: {
        ULONG inLen = ioStack->Parameters.DeviceIoControl.InputBufferLength;
        if (!buf || inLen == 0 || (inLen & 1)) return CompleteIrp(Irp, STATUS_INVALID_PARAMETER, 0);
        /* 用户态收到的是 ERROR_NOT_READY，mic-bridge 把它当良性（没人在录）。 */
        if (!DdMicRingIsActive()) return CompleteIrp(Irp, STATUS_DEVICE_NOT_READY, 0);
        return CompleteIrp(Irp, STATUS_SUCCESS, DdMicRingWrite(buf, inLen));
    }
    case IOCTL_DD_MIC_GET_STATS:
        if (!buf || ioStack->Parameters.DeviceIoControl.OutputBufferLength < sizeof(DD_MIC_STATS))
            return CompleteIrp(Irp, STATUS_BUFFER_TOO_SMALL, 0);
        DdMicRingGetStats((PDD_MIC_STATS)buf);
        return CompleteIrp(Irp, STATUS_SUCCESS, sizeof(DD_MIC_STATS));
    default:
        return CompleteIrp(Irp, STATUS_INVALID_DEVICE_REQUEST, 0);
    }
}

VOID DdMicControlDeviceHookDispatch(PDRIVER_OBJECT DriverObject)
{
    DriverObject->MajorFunction[IRP_MJ_CREATE]         = DdMicCtlIrpCreateClose;
    DriverObject->MajorFunction[IRP_MJ_CLOSE]          = DdMicCtlIrpCreateClose;
    DriverObject->MajorFunction[IRP_MJ_DEVICE_CONTROL] = DdMicCtlIrpDeviceControl;
    /* PcInitializeAdapterDriver 装过的其余 major（PNP / POWER / SYSTEM_CONTROL 只会发给
     * PnP 设备，不用管）。 */
    DriverObject->MajorFunction[IRP_MJ_READ]           = DdMicCtlIrpOther;
    DriverObject->MajorFunction[IRP_MJ_WRITE]          = DdMicCtlIrpOther;
    DriverObject->MajorFunction[IRP_MJ_FLUSH_BUFFERS]  = DdMicCtlIrpOther;
    DriverObject->MajorFunction[IRP_MJ_QUERY_SECURITY] = DdMicCtlIrpOther;
    DriverObject->MajorFunction[IRP_MJ_SET_SECURITY]   = DdMicCtlIrpOther;
}

NTSTATUS DdMicControlDeviceCreate(PDRIVER_OBJECT DriverObject, PDEVICE_OBJECT OwnerFdo)
{
    if (g_CtlDeviceObject) return STATUS_SUCCESS;

    UNICODE_STRING devName, linkName;
    RtlInitUnicodeString(&devName, L"\\Device\\DreamDeskMicCtl");
    RtlInitUnicodeString(&linkName, L"\\DosDevices\\DreamDeskMicCtl");

    /* 只有 SYSTEM 和管理员能开：谁能开谁就能往这台机器的麦克风里灌声音。
     * mic-bridge 是服务在会话 0 以 SYSTEM 拉起的。 */
    PDEVICE_OBJECT dev = NULL;
    NTSTATUS status = IoCreateDeviceSecure(DriverObject, 0, &devName, DD_MIC_DEVICE_TYPE,
                                           FILE_DEVICE_SECURE_OPEN, FALSE,
                                           &SDDL_DEVOBJ_SYS_ALL_ADM_ALL, NULL, &dev);
    if (!NT_SUCCESS(status)) return status;
    dev->Flags |= DO_BUFFERED_IO;

    status = IoCreateSymbolicLink(&linkName, &devName);
    if (!NT_SUCCESS(status)) {
        IoDeleteDevice(dev);
        return status;
    }
    dev->Flags &= ~DO_DEVICE_INITIALIZING;
    g_CtlOwnerFdo = OwnerFdo;
    g_CtlDeviceObject = dev;
    return STATUS_SUCCESS;
}

VOID DdMicControlDeviceDestroy(PDEVICE_OBJECT OwnerFdo)
{
    if (!g_CtlDeviceObject || OwnerFdo != g_CtlOwnerFdo) return;
    UNICODE_STRING linkName;
    RtlInitUnicodeString(&linkName, L"\\DosDevices\\DreamDeskMicCtl");
    IoDeleteSymbolicLink(&linkName);
    PDEVICE_OBJECT dev = g_CtlDeviceObject;
    g_CtlDeviceObject = NULL;
    g_CtlOwnerFdo = NULL;
    IoDeleteDevice(dev);
}
