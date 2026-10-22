/*
 * wasapi_capture.c - minimal WASAPI shared-mode capture from
 * "DreamDesk Virtual Audio" microphone endpoint. Drops APO chain dependency:
 * the only filtering applied is whatever audio engine itself does
 * for shared-mode SRC. If this output is clean speech but the
 * Voice Recorder app produces noise, the bug is in the recorder
 * app's APO/SRC, not the driver.
 *
 * Build:  cl /nologo /W3 /O2 /std:c17 wasapi_capture.c ole32.lib
 * Run:    wasapi_capture.exe <out.wav> <seconds> [raw|excl]
 *         raw  = AUDCLNT_STREAMOPTIONS_RAW, bypasses the APO chain.
 *         excl = exclusive mode, reads the driver's buffer with no engine in between
 *                (bit-exact; shared mode is silenced when no user is logged on).
 */

#define WIN32_LEAN_AND_MEAN
#define COBJMACROS
#include <windows.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <functiondiscoverykeys_devpkey.h>
#include <endpointvolume.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>

/* Manually instantiate the GUIDs we use; they are declared EXTERN_C const
 * in the headers but only defined in the older mmdevapi.lib that newer
 * SDKs no longer ship a static lib for. */
static const GUID DD_CLSID_MMDeviceEnumerator =
    {0xBCDE0395,0xE52F,0x467C,{0x8E,0x3D,0xC4,0x57,0x92,0x91,0x69,0x2E}};
static const GUID DD_IID_IMMDeviceEnumerator =
    {0xA95664D2,0x9614,0x4F35,{0xA7,0x46,0xDE,0x8D,0xB6,0x36,0x17,0xE6}};
static const GUID DD_IID_IAudioClient =
    {0x1CB9AD4C,0xDBFA,0x4C32,{0xB1,0x78,0xC2,0xF5,0x68,0xA7,0x03,0xB2}};
static const GUID DD_IID_IAudioCaptureClient =
    {0xC8ADBD64,0xE71E,0x48A0,{0xA4,0xDE,0x18,0x5C,0x39,0x5C,0xD3,0x17}};
static const GUID DD_IID_IAudioClient2 =
    {0x726778CD,0xF60A,0x4EDA,{0x82,0xDE,0xE4,0x76,0x10,0xCD,0x78,0xAA}};
static const GUID DD_IID_IAudioEndpointVolume =
    {0x5CDF2C82,0x841E,0x4546,{0x97,0x22,0x0C,0xF7,0x40,0x78,0x22,0x9A}};
static const GUID DD_KSDATAFORMAT_SUBTYPE_PCM =
    {0x00000001,0x0000,0x0010,{0x80,0x00,0x00,0xAA,0x00,0x38,0x9B,0x71}};
#define CLSID_MMDeviceEnumerator DD_CLSID_MMDeviceEnumerator
#define IID_IMMDeviceEnumerator  DD_IID_IMMDeviceEnumerator
#define IID_IAudioClient         DD_IID_IAudioClient
#define IID_IAudioCaptureClient  DD_IID_IAudioCaptureClient

#define HR_OK(hr) do { if (FAILED(hr)) { fprintf(stderr,"FAIL line %d hr=0x%08lx\n", __LINE__, (unsigned long)(hr)); return 1; } } while(0)

static int find_dd_mic(IMMDeviceEnumerator *en, IMMDevice **out) {
    IMMDeviceCollection *col = NULL;
    HRESULT hr = en->lpVtbl->EnumAudioEndpoints(en, eCapture, DEVICE_STATE_ACTIVE, &col);
    if (FAILED(hr)) return 1;
    UINT cnt = 0;
    col->lpVtbl->GetCount(col, &cnt);
    for (UINT i = 0; i < cnt; ++i) {
        IMMDevice *dev = NULL;
        col->lpVtbl->Item(col, i, &dev);
        IPropertyStore *ps = NULL;
        if (SUCCEEDED(dev->lpVtbl->OpenPropertyStore(dev, STGM_READ, &ps))) {
            PROPVARIANT pv; PropVariantInit(&pv);
            if (SUCCEEDED(ps->lpVtbl->GetValue(ps, &PKEY_Device_FriendlyName, &pv))) {
                if (pv.vt == VT_LPWSTR && wcsstr(pv.pwszVal, L"DreamDesk")) {
                    fwprintf(stderr, L"found: %s\n", pv.pwszVal);
                    PropVariantClear(&pv); ps->lpVtbl->Release(ps);
                    *out = dev;
                    col->lpVtbl->Release(col);
                    return 0;
                }
                PropVariantClear(&pv);
            }
            ps->lpVtbl->Release(ps);
        }
        dev->lpVtbl->Release(dev);
    }
    col->lpVtbl->Release(col);
    return 1;
}

static void write_wav_header(FILE *f, uint32_t sr, uint16_t ch, uint16_t bps, uint32_t data_bytes) {
    fwrite("RIFF", 1, 4, f);
    uint32_t riff_sz = 36 + data_bytes; fwrite(&riff_sz, 4, 1, f);
    fwrite("WAVEfmt ", 1, 8, f);
    uint32_t fmt_sz = 16; fwrite(&fmt_sz, 4, 1, f);
    uint16_t fmt = 1; fwrite(&fmt, 2, 1, f);  /* PCM */
    fwrite(&ch, 2, 1, f);
    fwrite(&sr, 4, 1, f);
    uint32_t br = sr * ch * bps / 8; fwrite(&br, 4, 1, f);
    uint16_t ba = ch * bps / 8; fwrite(&ba, 2, 1, f);
    fwrite(&bps, 2, 1, f);
    fwrite("data", 1, 4, f);
    fwrite(&data_bytes, 4, 1, f);
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr,"usage: %s out.wav seconds [raw]\n", argv[0]); return 1; }
    const char *out_path = argv[1];
    int seconds = atoi(argv[2]);
    if (seconds <= 0) seconds = 5;
    int raw = argc > 3 && strcmp(argv[3], "raw") == 0;
    int excl = argc > 3 && strcmp(argv[3], "excl") == 0;

    HRESULT hr = CoInitialize(NULL); HR_OK(hr);

    IMMDeviceEnumerator *en = NULL;
    hr = CoCreateInstance(&CLSID_MMDeviceEnumerator, NULL, CLSCTX_ALL,
                          &IID_IMMDeviceEnumerator, (void**)&en);
    HR_OK(hr);

    IMMDevice *dev = NULL;
    if (find_dd_mic(en, &dev) != 0) {
        fprintf(stderr, "DreamDesk Virtual Audio microphone not found\n");
        return 1;
    }

    IAudioEndpointVolume *vol = NULL;
    if (SUCCEEDED(dev->lpVtbl->Activate(dev, &DD_IID_IAudioEndpointVolume, CLSCTX_ALL, NULL, (void**)&vol))) {
        BOOL mute = FALSE; float lvl = 0;
        vol->lpVtbl->GetMute(vol, &mute);
        vol->lpVtbl->GetMasterVolumeLevelScalar(vol, &lvl);
        fprintf(stderr, "endpoint mute=%d volume=%.2f\n", mute, lvl);
        vol->lpVtbl->Release(vol);
    }

    IAudioClient *ac = NULL;
    hr = dev->lpVtbl->Activate(dev, raw ? &DD_IID_IAudioClient2 : &IID_IAudioClient,
                               CLSCTX_ALL, NULL, (void**)&ac);
    HR_OK(hr);
    if (raw) {
        AudioClientProperties props = {0};
        props.cbSize = sizeof(props);
        props.eCategory = AudioCategory_Other;
        props.Options = AUDCLNT_STREAMOPTIONS_RAW;
        hr = ((IAudioClient2 *)ac)->lpVtbl->SetClientProperties((IAudioClient2 *)ac, &props);
        HR_OK(hr);
        fprintf(stderr, "raw mode requested\n");
    }

    WAVEFORMATEX *wf = NULL;
    if (excl) {
        /* The driver's only format; exclusive mode reads its buffer with no engine in between. */
        WAVEFORMATEXTENSIBLE *x = CoTaskMemAlloc(sizeof(*x));
        if (!x) return 1;
        ZeroMemory(x, sizeof(*x));
        x->Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE;
        x->Format.nChannels = 1;
        x->Format.nSamplesPerSec = 48000;
        x->Format.wBitsPerSample = 16;
        x->Format.nBlockAlign = 2;
        x->Format.nAvgBytesPerSec = 96000;
        x->Format.cbSize = sizeof(*x) - sizeof(WAVEFORMATEX);
        x->Samples.wValidBitsPerSample = 16;
        x->dwChannelMask = SPEAKER_FRONT_CENTER;
        x->SubFormat = DD_KSDATAFORMAT_SUBTYPE_PCM;
        wf = &x->Format;
        REFERENCE_TIME hns = 1000000; /* 100 ms, polling: a short buffer turns our own stalls into gaps */
        hr = ac->lpVtbl->Initialize(ac, AUDCLNT_SHAREMODE_EXCLUSIVE, 0, hns, hns, wf, NULL);
        HR_OK(hr);
        fprintf(stderr, "exclusive mode: 48000 Hz 1 ch 16 bits\n");
    } else {
        hr = ac->lpVtbl->GetMixFormat(ac, &wf);
        HR_OK(hr);
        fprintf(stderr, "engine mix format: %u Hz %u ch %u bits (fmtTag=0x%04x)\n",
                wf->nSamplesPerSec, wf->nChannels, wf->wBitsPerSample, wf->wFormatTag);

        REFERENCE_TIME hns = 10000000; /* 1 s buffer */
        hr = ac->lpVtbl->Initialize(ac, AUDCLNT_SHAREMODE_SHARED, 0, hns, 0, wf, NULL);
        HR_OK(hr);
    }

    IAudioCaptureClient *cc = NULL;
    hr = ac->lpVtbl->GetService(ac, &IID_IAudioCaptureClient, (void**)&cc);
    HR_OK(hr);

    FILE *f = fopen(out_path, "wb");
    if (!f) { fprintf(stderr, "cannot open %s\n", out_path); return 1; }
    /* Reserve header space; rewrite at end. */
    write_wav_header(f, wf->nSamplesPerSec, wf->nChannels, wf->wBitsPerSample, 0);

    hr = ac->lpVtbl->Start(ac); HR_OK(hr);

    DWORD start = GetTickCount();
    uint32_t total_bytes = 0;
    unsigned pkts = 0, silent_pkts = 0, discont_pkts = 0;
    while (GetTickCount() - start < (DWORD)(seconds * 1000)) {
        UINT32 pkt_frames = 0;
        cc->lpVtbl->GetNextPacketSize(cc, &pkt_frames);
        while (pkt_frames > 0) {
            BYTE *data = NULL;
            UINT32 frames = 0;
            DWORD flags = 0;
            hr = cc->lpVtbl->GetBuffer(cc, &data, &frames, &flags, NULL, NULL);
            if (FAILED(hr)) break;
            pkts++;
            if (flags & AUDCLNT_BUFFERFLAGS_SILENT) silent_pkts++;
            if (flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY) discont_pkts++;
            UINT32 bytes = frames * wf->nBlockAlign;
            if (!(flags & AUDCLNT_BUFFERFLAGS_SILENT) && data && bytes) {
                fwrite(data, 1, bytes, f);
                total_bytes += bytes;
            } else if (flags & AUDCLNT_BUFFERFLAGS_SILENT) {
                /* zero-fill silent packets so timeline matches */
                static const BYTE zeros[4096] = {0};
                UINT32 left = bytes;
                while (left) {
                    UINT32 chunk = left > sizeof(zeros) ? sizeof(zeros) : left;
                    fwrite(zeros, 1, chunk, f);
                    left -= chunk;
                }
                total_bytes += bytes;
            }
            cc->lpVtbl->ReleaseBuffer(cc, frames);
            cc->lpVtbl->GetNextPacketSize(cc, &pkt_frames);
        }
        Sleep(5);
    }

    ac->lpVtbl->Stop(ac);
    /* Rewrite WAV header with correct size. */
    fseek(f, 0, SEEK_SET);
    write_wav_header(f, wf->nSamplesPerSec, wf->nChannels, wf->wBitsPerSample, total_bytes);
    fclose(f);
    fprintf(stderr, "captured %u bytes (%.1fs) packets=%u silent=%u discontinuity=%u\n",
            total_bytes,
            (double)total_bytes / (wf->nSamplesPerSec * wf->nBlockAlign),
            pkts, silent_pkts, discont_pkts);

    cc->lpVtbl->Release(cc);
    CoTaskMemFree(wf);
    ac->lpVtbl->Release(ac);
    dev->lpVtbl->Release(dev);
    en->lpVtbl->Release(en);
    CoUninitialize();
    return 0;
}
