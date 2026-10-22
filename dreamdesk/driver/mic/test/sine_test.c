/*
 * sine_test.c - push a 1 kHz sine into the DreamDesk mic via IOCTL_DD_MIC_WRITE_PCM.
 * Record the "DreamDesk Virtual Audio" microphone (wasapi_capture.exe) while this runs. Needs admin.
 *
 * Build:  cl /nologo /W4 /O2 sine_test.c winmm.lib
 * Run:    sine_test.exe [ramp]
 */

#define _USE_MATH_DEFINES
#include <windows.h>
#include <stdio.h>
#include <math.h>
#include <string.h>

#include "../include/dd_mic_ioctl.h"

#define SAMPLE_RATE      48000
#define FRAMES_PER_PKT   240                 /* 5 ms, same packet size as mic-bridge */
#define PREFILL_FRAMES   480                 /* stay ~10 ms ahead of the engine */

/* sine_test.exe ramp: push a counter (+1 per sample, wraps) instead of a sine, so every
 * glitch in a bit-exact capture classifies itself: zeros then +1 = underrun, forward jump
 * = dropped samples, backward jump = stale buffer read. */
int main(int argc, char **argv) {
    int ramp = argc > 1 && strcmp(argv[1], "ramp") == 0;
    unsigned short counter = 1;
    HANDLE dev = CreateFileA("\\\\.\\DreamDeskMicCtl", GENERIC_WRITE,
                             FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING, 0, NULL);
    if (dev == INVALID_HANDLE_VALUE) {
        fprintf(stderr, "open \\\\.\\DreamDeskMicCtl failed: err=%lu (driver loaded? admin?)\n",
                GetLastError());
        return 1;
    }

    static short pcm[FRAMES_PER_PKT];
    const double tau = 2.0 * M_PI * 1000.0 / SAMPLE_RATE;
    double phase = 0.0;

    /* Produce exactly what the wall clock owes. Sleep-paced pushing with the
     * default 15.6 ms timer under-produces and reads as driver dropouts. */
    timeBeginPeriod(1);
    LARGE_INTEGER qpf, t0, now;
    QueryPerformanceFrequency(&qpf);
    QueryPerformanceCounter(&t0);
    LONGLONG produced = 0;

    fprintf(stdout, "Pushing 1 kHz sine into DreamDesk Mic. Ctrl-C to stop.\n");
    fflush(stdout);

    /* Once a second: our own worst gap between loop turns. A stall longer than the driver's
     * start threshold is an underrun this tool caused, not the driver. */
    LONGLONG last = t0.QuadPart, worst = 0, report_at = t0.QuadPart + qpf.QuadPart;
    for (;;) {
        QueryPerformanceCounter(&now);
        if (now.QuadPart - last > worst) worst = now.QuadPart - last;
        last = now.QuadPart;
        if (now.QuadPart >= report_at) {
            fprintf(stdout, "worst_loop_stall_ms=%.1f\n", worst * 1000.0 / qpf.QuadPart);
            fflush(stdout);
            worst = 0;
            report_at += qpf.QuadPart;
        }
        LONGLONG owed = (now.QuadPart - t0.QuadPart) * SAMPLE_RATE / qpf.QuadPart
                        + PREFILL_FRAMES - produced;
        while (owed >= FRAMES_PER_PKT) {
            for (int i = 0; i < FRAMES_PER_PKT; ++i) {
                if (ramp) {
                    if (++counter == 0) counter = 1;   /* keep 0 for "the driver zero-filled" */
                    pcm[i] = (short)counter;
                    continue;
                }
                pcm[i] = (short)(16000.0 * sin(phase));
                phase += tau;
                if (phase > 2 * M_PI) phase -= 2 * M_PI;
            }
            DWORD ret = 0;
            if (!DeviceIoControl(dev, IOCTL_DD_MIC_WRITE_PCM, pcm, (DWORD)sizeof(pcm),
                                 NULL, 0, &ret, NULL) &&
                GetLastError() != ERROR_NOT_READY) {   /* NOT_READY = nobody recording yet */
                fprintf(stderr, "DeviceIoControl: err=%lu\n", GetLastError());
                return 1;
            }
            produced += FRAMES_PER_PKT;
            owed -= FRAMES_PER_PKT;
        }
        Sleep(1);
    }
}
