/*
 * stats_query.c - poll DreamDeskMic ring stats once per second so we
 * can tell whether the kernel-side audio engine is actually pulling
 * PCM out of the ring while dd_mic_bridge.exe is pushing into it.
 *
 * Three counters from DD_MIC_STATS:
 *   - bytes_consumed_total : driver increments as audio engine reads
 *                            samples from the ring (= proof the
 *                            WriteBytes hook fires).
 *   - underrun_bytes       : engine read while ring empty (consumer
 *                            faster than producer, OR producer dead).
 *   - overrun_bytes        : producer wrote with no room (= ring is
 *                            full and consumer not draining).
 *
 * Build (Windows, MSVC):
 *   cl /nologo /W3 /O2 /std:c17 stats_query.c
 *
 * Run:  stats_query.exe
 * Stop: Ctrl-C
 */

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winioctl.h>
#include <stdio.h>

#include "../include/dd_mic_ioctl.h"

int main(void) {
    HANDLE dev = CreateFileA("\\\\.\\DreamDeskMicCtl",
                              GENERIC_READ,
                              FILE_SHARE_READ | FILE_SHARE_WRITE,
                              NULL, OPEN_EXISTING, 0, NULL);
    if (dev == INVALID_HANDLE_VALUE) {
        fprintf(stderr, "Failed to open \\\\.\\DreamDeskMicCtl: err=%lu\n",
                GetLastError());
        return 1;
    }

    DD_MIC_STATS prev = {0};
    DD_MIC_STATS now  = {0};
    BOOL first = TRUE;

    fprintf(stdout,
            "Polling DreamDeskMic stats. Ctrl-C to stop.\n"
            "(consumed=engine-pulled, underrun=engine-read-empty, overrun=ring-full)\n\n");
    fflush(stdout);

    for (;;) {
        DWORD ret = 0;
        BOOL ok = DeviceIoControl(dev,
                                   IOCTL_DD_MIC_GET_STATS,
                                   NULL, 0,
                                   &now, sizeof(now),
                                   &ret, NULL);
        if (!ok || ret != sizeof(now)) {
            fprintf(stderr, "GET_STATS err=%lu ret=%lu\n",
                    GetLastError(), ret);
            CloseHandle(dev);
            return 1;
        }
        if (first) {
            fprintf(stdout, "init  consumed=%ld underrun=%ld overrun=%ld\n",
                    (long)now.bytes_consumed_total,
                    (long)now.underrun_bytes,
                    (long)now.overrun_bytes);
            first = FALSE;
        } else {
            long dc = (long)(now.bytes_consumed_total - prev.bytes_consumed_total);
            long du = (long)(now.underrun_bytes - prev.underrun_bytes);
            long doo = (long)(now.overrun_bytes - prev.overrun_bytes);
            fprintf(stdout,
                    "+1s  consumed+=%ld underrun+=%ld overrun+=%ld   total c=%ld u=%ld o=%ld\n",
                    dc, du, doo,
                    (long)now.bytes_consumed_total,
                    (long)now.underrun_bytes,
                    (long)now.overrun_bytes);
        }
        fflush(stdout);
        prev = now;
        Sleep(1000);
    }
}
