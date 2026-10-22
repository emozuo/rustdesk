/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Module Name:

    Definitions.h

Abstract:

    Header file for common stuff.
--*/

#ifndef _SIMPLEAUDIOSAMPLE_DEFINITIONS_H_
#define _SIMPLEAUDIOSAMPLE_DEFINITIONS_H_

#include <portcls.h>
#include <stdunk.h>
#include <ksdebug.h>
#include <ntintsafe.h>
#include <wdf.h>
#include <wdfminiport.h>
#include <Ntstrsafe.h>
#include "NewDelete.h"

//=============================================================================
// Defines
//=============================================================================

// Pool tag used for SIMPLEAUDIOSAMPLE allocations
#define SIMPLEAUDIOSAMPLE_POOLTAG               'SASM'  

// Debug module name
#define STR_MODULENAME              "SIMPLEAUDIOSAMPLE: "

// Debug utility macros
#define D_FUNC                      4
#define D_BLAB                      DEBUGLVL_BLAB
#define D_VERBOSE                   DEBUGLVL_VERBOSE        
#define D_TERSE                     DEBUGLVL_TERSE          
#define D_ERROR                     DEBUGLVL_ERROR          
#define DPF                         _DbgPrintF
#define DPF_ENTER(x)                DPF(D_FUNC, x)

// Channel orientation
#define CHAN_LEFT                   0
#define CHAN_RIGHT                  1
#define CHAN_MASTER                 (-1)

// Dma Settings.
#define DMA_BUFFER_SIZE             0x16000

#define _100NS_PER_MILLISECOND      10000                       // number of 100ns units per millisecond

// Macros to assist with pin instance counting
#define VERIFY_PIN_INSTANCE_RESOURCES_AVAILABLE(status, allocated, max) \
    status = (allocated < max) ? STATUS_SUCCESS : STATUS_INSUFFICIENT_RESOURCES

#define ALLOCATE_PIN_INSTANCE_RESOURCES(allocated) \
    allocated++

#define FREE_PIN_INSTANCE_RESOURCES(allocated) \
    allocated--

// Define the value data type for supported sound detector patterns. Only
// one pattern type is supported in this sample.
typedef struct {
    KSMULTIPLE_ITEM MultipleItem;
    GUID            PatternType[1];
} CONTOSO_SUPPORTEDPATTERNSVALUE;

//=============================================================================
// Typedefs
//=============================================================================

// Flags to identify stream processing mode
typedef enum {
    CONNECTIONTYPE_TOPOLOGY_OUTPUT = 0,
    CONNECTIONTYPE_WAVE_OUTPUT     = 1
} CONNECTIONTYPE;

// Connection table for registering topology/wave bridge connection
typedef struct _PHYSICALCONNECTIONTABLE
{
    ULONG            ulTopology;
    ULONG            ulWave;
    CONNECTIONTYPE   eType;
} PHYSICALCONNECTIONTABLE, *PPHYSICALCONNECTIONTABLE;

//
// This is the structure of the portclass FDO device extension Nt has created
// for us.  We keep the adapter common object here.
//
struct IAdapterCommon;
typedef struct _PortClassDeviceContext              // 32       64      Byte offsets for 32 and 64 bit architectures
{
    ULONG_PTR m_pulReserved1[2];                    // 0-7      0-15    First two pointers are reserved.
    PDEVICE_OBJECT m_DoNotUsePhysicalDeviceObject;  // 8-11     16-23   Reserved pointer to our Physical Device Object (PDO).
    PVOID m_pvReserved2;                            // 12-15    24-31   Reserved pointer to our Start Device function.
    PVOID m_pvReserved3;                            // 16-19    32-39   "Out Memory" according to DDK.  
    IAdapterCommon* m_pCommon;                      // 20-23    40-47   Pointer to our adapter common object.
    PVOID m_pvUnused1;                              // 24-27    48-55   Unused space.
    PVOID m_pvUnused2;                              // 28-31    56-63   Unused space.

    // Anything after above line should not be used.
    // This actually goes on for (64*sizeof(ULONG_PTR)) but it is all opaque.
} PortClassDeviceContext;

//
// Major/MinorTarget to object casting.
//
#define MajorTarget_to_Obj(ptr) \
    reinterpret_cast<CMiniportWaveRT*>(ptr)
    
#define MinorTarget_to_Obj(ptr) \
    static_cast<CMiniportWaveRTStream*>(reinterpret_cast<PMINIPORTWAVERTSTREAM>(ptr))

//=============================================================================
// Function prototypes
//=============================================================================

// Default WaveFilter automation table.
// Handles the GeneralComponentId request.
NTSTATUS PropertyHandler_WaveFilter
(
    _In_ PPCPROPERTY_REQUEST PropertyRequest
);

NTSTATUS PropertyHandler_GenericPin
( 
    _In_ PPCPROPERTY_REQUEST      PropertyRequest 
);

// common.h uses some of the above definitions.
#include "common.h"
#include "kshelper.h"

#endif // _SIMPLEAUDIOSAMPLE_DEFINITIONS_H_
