/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Module Name:

    minipairs.h

Abstract:

    Local audio endpoint filter definitions.
--*/

#ifndef _SIMPLEAUDIOSAMPLE_MINIPAIRS_H_
#define _SIMPLEAUDIOSAMPLE_MINIPAIRS_H_

#include "speakertopo.h"
#include "speakertoptable.h"
#include "speakerwavtable.h"

#include "micarraytopo.h"
#include "micarray1toptable.h"
#include "micarraywavtable.h"


NTSTATUS
CreateMiniportWaveRTSimpleAudioSample
(
    _Out_       PUNKNOWN *,
    _In_        REFCLSID,
    _In_opt_    PUNKNOWN,
    _In_        POOL_FLAGS,
    _In_        PUNKNOWN,
    _In_opt_    PVOID,
    _In_        PENDPOINT_MINIPAIR
);

NTSTATUS
CreateMiniportTopologySimpleAudioSample
(
    _Out_       PUNKNOWN *,
    _In_        REFCLSID,
    _In_opt_    PUNKNOWN,
    _In_        POOL_FLAGS,
    _In_        PUNKNOWN,
    _In_opt_    PVOID,
    _In_        PENDPOINT_MINIPAIR
);

//
// Render miniports.
//

/*********************************************************************
* Topology/Wave bridge connection for speaker                        *
*                                                                    *
*              +------+                +------+                      *
*              | Wave |                | Topo |                      *
*              |      |                |      |                      *
* System   --->|0    1|--------------->|0    1|---> Line Out         *
*              |      |                |      |                      *
*              +------+                +------+                      *
*********************************************************************/
static
PHYSICALCONNECTIONTABLE SpeakerTopologyPhysicalConnections[] =
{
    {
        KSPIN_TOPO_WAVEOUT_SOURCE,  // TopologyIn
        KSPIN_WAVE_RENDER3_SOURCE,  // WaveOut
        CONNECTIONTYPE_WAVE_OUTPUT
    }
};

// 被控端没有真声卡时远程照样有声：会话期间设成默认播放设备，被控端从它 loopback 采集。
// 渲染数据驱动直接丢掉（样例的 SaveData 落盘删了）。格式只有 48k/16/立体声，
// 正好是 dd_audio_bridge 要的混音速率。
static
ENDPOINT_MINIPAIR DreamDeskSpeakerMiniports =
{
    eSpeakerDevice,
    L"TopologyDreamDeskSpeaker",            // make sure this matches KSNAME_TopologyDreamDeskSpeaker in the inf's [Strings] section
    NULL,                                   // optional template name
    CreateMiniportTopologySimpleAudioSample,
    &SpeakerTopoMiniportFilterDescriptor,
    0, NULL,                                // Interface properties
    L"WaveDreamDeskSpeaker",                // make sure this matches KSNAME_WaveDreamDeskSpeaker in the inf's [Strings] section
    NULL,                                   // optional template name
    CreateMiniportWaveRTSimpleAudioSample,
    &SpeakerWaveMiniportFilterDescriptor,
    0,                                      // Interface properties
    NULL,
    SpeakerPinDeviceFormatsAndModes,
    SIZEOF_ARRAY(SpeakerPinDeviceFormatsAndModes),
    SpeakerTopologyPhysicalConnections,
    SIZEOF_ARRAY(SpeakerTopologyPhysicalConnections),
    ENDPOINT_NO_FLAGS,
};

//
// Capture miniports.
//

/*********************************************************************
* Topology/Wave bridge connection for mic array  1 (front)           *
*                                                                    *
*              +------+    +------+                                  *
*              | Topo |    | Wave |                                  *
*              |      |    |      |                                  *
*  Mic in  --->|0    1|===>|0    1|---> Capture Host Pin             *
*              |      |    |      |                                  *
*              +------+    +------+                                  *
*********************************************************************/
static
PHYSICALCONNECTIONTABLE MicArray1TopologyPhysicalConnections[] =
{
    {
        KSPIN_TOPO_BRIDGE,          // TopologyOut
        KSPIN_WAVE_BRIDGE,          // WaveIn
        CONNECTIONTYPE_TOPOLOGY_OUTPUT
    }
};

static
ENDPOINT_MINIPAIR DreamDeskMicMiniports =
{
    eMicArrayDevice1,
    L"TopologyDreamDeskMic",                // make sure this matches KSNAME_TopologyDreamDeskMic in the inf's [Strings] section
    NULL,                                   // optional template name
    CreateMicArrayMiniportTopology,
    &MicArray1TopoMiniportFilterDescriptor,
    0, NULL,                                // Interface properties
    L"WaveDreamDeskMic",                    // make sure this matches KSNAME_WaveDreamDeskMic in the inf's [Strings] section
    NULL,                                   // optional template name
    CreateMiniportWaveRTSimpleAudioSample,
    &MicArrayWaveMiniportFilterDescriptor,
    0,                                      // Interface properties
    NULL,
    MicArrayPinDeviceFormatsAndModes,
    SIZEOF_ARRAY(MicArrayPinDeviceFormatsAndModes),
    MicArray1TopologyPhysicalConnections,
    SIZEOF_ARRAY(MicArray1TopologyPhysicalConnections),
    ENDPOINT_NO_FLAGS,
};

//=============================================================================
//
// 全部端点。样例分成渲染 / 采集两个数组，它自己也注明那是随意的，这里合成一个。
//
static
PENDPOINT_MINIPAIR  g_Endpoints[] =
{
    &DreamDeskSpeakerMiniports,
    &DreamDeskMicMiniports,
};

#define g_cEndpoints (SIZEOF_ARRAY(g_Endpoints))

//=============================================================================
//
// Total miniports = # endpoints * 2 (topology + wave).
//
#define g_MaxMiniports  (g_cEndpoints * 2)

#endif // _SIMPLEAUDIOSAMPLE_MINIPAIRS_H_
