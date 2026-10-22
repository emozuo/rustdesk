
/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Module Name:

    basetopo.h

Abstract:

    Declaration of topology miniport.
--*/

#ifndef _SIMPLEAUDIOSAMPLE_BASETOPO_H_
#define _SIMPLEAUDIOSAMPLE_BASETOPO_H_

//=============================================================================
// Classes
//=============================================================================

///////////////////////////////////////////////////////////////////////////////
// CMiniportTopologySimpleAudioSample
//

class CMiniportTopologySimpleAudioSample
{
  protected:
    PADAPTERCOMMON              m_AdapterCommon;        // Adapter common object.
    PPCFILTER_DESCRIPTOR        m_FilterDescriptor;     // Filter descriptor.
    PPORTEVENTS                 m_PortEvents;           // Event interface.

  public:
    CMiniportTopologySimpleAudioSample(
        _In_        PCFILTER_DESCRIPTOR    *FilterDesc
        );
    
    ~CMiniportTopologySimpleAudioSample();

    NTSTATUS                    GetDescription
    (   
        _Out_ PPCFILTER_DESCRIPTOR *  Description
    );

    NTSTATUS                    DataRangeIntersection
    (   
        _In_  ULONG             PinId,
        _In_  PKSDATARANGE      ClientDataRange,
        _In_  PKSDATARANGE      MyDataRange,
        _In_  ULONG             OutputBufferLength,
        _Out_writes_bytes_to_opt_(OutputBufferLength, *ResultantFormatLength)
              PVOID             ResultantFormat OPTIONAL,
        _Out_ PULONG            ResultantFormatLength
    );

    NTSTATUS                    Init
    ( 
        _In_  PUNKNOWN          UnknownAdapter,
        _In_  PPORTTOPOLOGY     Port_ 
    );

    VOID                        AddEventToEventList
    (
        _In_  PKSEVENT_ENTRY    EventEntry 
    );
    
    VOID                        GenerateEventList
    (
        _In_opt_    GUID       *Set,
        _In_        ULONG       EventId,
        _In_        BOOL        PinEvent,
        _In_        ULONG       PinId,
        _In_        BOOL        NodeEvent,
        _In_        ULONG       NodeId
    );
};

#endif
