/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Module Name:

    micarray1toptable.h

Abstract:

    Declaration of topology tables for the mic array.

--*/

#ifndef _SIMPLEAUDIOSAMPLE_MICARRAY1TOPTABLE_H_
#define _SIMPLEAUDIOSAMPLE_MICARRAY1TOPTABLE_H_

//
// {6ae81ff4-203e-4fe1-88aa-f2d57775cd4a}
DEFINE_GUID(MICARRAY1_CUSTOM_NAME,
    0x6ae81ff4, 0x203e, 0x4fe1, 0x88, 0xaa, 0xf2, 0xd5, 0x77, 0x75, 0xcd, 0x4a);

//=============================================================================
static
KSDATARANGE MicArray1TopoPinDataRangesBridge[] =
{
 {
   sizeof(KSDATARANGE),
   0,
   0,
   0,
   STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
   STATICGUIDOF(KSDATAFORMAT_SUBTYPE_ANALOG),
   STATICGUIDOF(KSDATAFORMAT_SPECIFIER_NONE)
 }
};

//=============================================================================
static
PKSDATARANGE MicArray1TopoPinDataRangePointersBridge[] =
{
  &MicArray1TopoPinDataRangesBridge[0]
};

//=============================================================================
static
PCPIN_DESCRIPTOR MicArray1TopoMiniportPins[] =
{
    // KSPIN_TOPO_MIC_ELEMENTS
    {
      0,
      0,
      0,                                              // InstanceCount
      NULL,                                           // AutomationTable
      {                                               // KsPinDescriptor
        0,                                            // InterfacesCount
        NULL,                                         // Interfaces
        0,                                            // MediumsCount
        NULL,                                         // Mediums
        SIZEOF_ARRAY(MicArray1TopoPinDataRangePointersBridge),     // DataRangesCount
        MicArray1TopoPinDataRangePointersBridge,                   // DataRanges
        KSPIN_DATAFLOW_IN,                            // DataFlow
        KSPIN_COMMUNICATION_NONE,                     // Communication
        &KSNODETYPE_MICROPHONE_ARRAY,                 // Category
        &MICARRAY1_CUSTOM_NAME,                       // Name
        0                                             // Reserved
      }
    },

    // KSPIN_TOPO_BRIDGE
    {
      0,
      0,
      0,                                              // InstanceCount
      NULL,                                           // AutomationTable
      {                                               // KsPinDescriptor
        0,                                            // InterfacesCount
        NULL,                                         // Interfaces
        0,                                            // MediumsCount
        NULL,                                         // Mediums
        SIZEOF_ARRAY(MicArray1TopoPinDataRangePointersBridge),     // DataRangesCount
        MicArray1TopoPinDataRangePointersBridge,                   // DataRanges
        KSPIN_DATAFLOW_OUT,                           // DataFlow
        KSPIN_COMMUNICATION_NONE,                     // Communication
        &KSCATEGORY_AUDIO,                            // Category
        NULL,                                         // Name
        0                                             // Reserved
      }
    }
};

//=============================================================================
// 真硬件音量：值由 DdMicRingRead 作用到样本上（理由见 dd_mic_ring.h）。扬声器不同 ——
// loopback 取在驱动之前，硬件音量对它无效，所以那边交给引擎的软件音量。
enum { KSNODE_MIC_VOLUME = 0, KSNODE_MIC_MUTE };

static
PCPROPERTY_ITEM MicArray1PropertiesVolume[] =
{
    {
        &KSPROPSETID_Audio,
        KSPROPERTY_AUDIO_VOLUMELEVEL,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_MicArrayLevel
    }
};

DEFINE_PCAUTOMATION_TABLE_PROP(AutomationMicArray1Volume, MicArray1PropertiesVolume);

static
PCPROPERTY_ITEM MicArray1PropertiesMute[] =
{
    {
        &KSPROPSETID_Audio,
        KSPROPERTY_AUDIO_MUTE,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_MicArrayLevel
    }
};

DEFINE_PCAUTOMATION_TABLE_PROP(AutomationMicArray1Mute, MicArray1PropertiesMute);

static
PCNODE_DESCRIPTOR MicArray1TopologyNodes[] =
{
    // KSNODE_MIC_VOLUME
    {
        0,
        &AutomationMicArray1Volume,
        &KSNODETYPE_VOLUME,
        &KSAUDFNAME_MIC_VOLUME
    },
    // KSNODE_MIC_MUTE
    {
        0,
        &AutomationMicArray1Mute,
        &KSNODETYPE_MUTE,
        &KSAUDFNAME_MIC_MUTE
    }
};

static
PCCONNECTION_DESCRIPTOR MicArray1TopoMiniportConnections[] =
{
    //  FromNode,                 FromPin,                    ToNode,                 ToPin
    {   PCFILTER_NODE,            KSPIN_TOPO_MIC_ELEMENTS,    KSNODE_MIC_VOLUME,      1 },
    {   KSNODE_MIC_VOLUME,        0,                          KSNODE_MIC_MUTE,        1 },
    {   KSNODE_MIC_MUTE,          0,                          PCFILTER_NODE,          KSPIN_TOPO_BRIDGE }
};


//=============================================================================
static
PCPROPERTY_ITEM MicArray1PropertiesTopoFilter[] =
{
    {
        &KSPROPSETID_Jack,
        KSPROPERTY_JACK_DESCRIPTION,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_MicArrayTopoFilter
    },
    {
        &KSPROPSETID_Jack,
        KSPROPERTY_JACK_DESCRIPTION2,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_MicArrayTopoFilter
    },
    {
        &KSPROPSETID_Audio,
        KSPROPERTY_AUDIO_MIC_ARRAY_GEOMETRY,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_MicArrayTopoFilter
    },
    {
        &KSPROPSETID_Audio,
        KSPROPERTY_AUDIO_MIC_SNR,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_MicArrayTopoFilter
    },
    {
        &KSPROPSETID_Audio,
        KSPROPERTY_AUDIO_MIC_SENSITIVITY2,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_MicArrayTopoFilter
    }
};

DEFINE_PCAUTOMATION_TABLE_PROP(AutomationMicArray1TopoFilter, MicArray1PropertiesTopoFilter);

//=============================================================================
static
PCFILTER_DESCRIPTOR MicArray1TopoMiniportFilterDescriptor =
{
  0,                                            // Version
  &AutomationMicArray1TopoFilter,               // AutomationTable
  sizeof(PCPIN_DESCRIPTOR),                     // PinSize
  SIZEOF_ARRAY(MicArray1TopoMiniportPins),      // PinCount
  MicArray1TopoMiniportPins,                    // Pins
  sizeof(PCNODE_DESCRIPTOR),                    // NodeSize
  SIZEOF_ARRAY(MicArray1TopologyNodes),         // NodeCount
  MicArray1TopologyNodes,                       // Nodes
  SIZEOF_ARRAY(MicArray1TopoMiniportConnections),// ConnectionCount
  MicArray1TopoMiniportConnections,             // Connections
  0,                                            // CategoryCount
  NULL                                          // Categories
};

#endif // _SIMPLEAUDIOSAMPLE_MICARRAY1TOPTABLE_H_
