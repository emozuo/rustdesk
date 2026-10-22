/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Module Name:

    kshelper.cpp

Abstract:

    Helper functions for simple audio sample
--*/

#include "definitions.h"

//4127: conditional expression is constant
#pragma warning (disable : 4127)

//-----------------------------------------------------------------------------
#pragma code_seg("PAGE")
PWAVEFORMATEX                   
GetWaveFormatEx
(
    _In_  PKSDATAFORMAT           pDataFormat
)
/*++

Routine Description:

  Returns the waveformatex for known formats. 

Arguments:

  pDataFormat - data format.

Return Value:
    
    waveformatex in DataFormat.
    NULL for unknown data formats.

--*/
{
    PAGED_CODE();

    PWAVEFORMATEX           pWfx = NULL;
    
    // If this is a known dataformat extract the waveformat info.
    //
    if
    ( 
        pDataFormat &&
        ( IsEqualGUIDAligned(pDataFormat->MajorFormat, 
                KSDATAFORMAT_TYPE_AUDIO)             &&
          ( IsEqualGUIDAligned(pDataFormat->Specifier, 
                KSDATAFORMAT_SPECIFIER_WAVEFORMATEX) ||
            IsEqualGUIDAligned(pDataFormat->Specifier, 
                KSDATAFORMAT_SPECIFIER_DSOUND) ) )
    )
    {
        pWfx = PWAVEFORMATEX(pDataFormat + 1);

        if (IsEqualGUIDAligned(pDataFormat->Specifier, 
                KSDATAFORMAT_SPECIFIER_DSOUND))
        {
            PKSDSOUND_BUFFERDESC    pwfxds;

            pwfxds = PKSDSOUND_BUFFERDESC(pDataFormat + 1);
            pWfx = &pwfxds->WaveFormatEx;
        }
    }

    return pWfx;
} // GetWaveFormatEx

//-----------------------------------------------------------------------------
#pragma code_seg("PAGE")
NTSTATUS 
ValidatePropertyParams
(
    _In_ PPCPROPERTY_REQUEST      PropertyRequest, 
    _In_ ULONG                    cbValueSize,
    _In_ ULONG                    cbInstanceSize /* = 0  */
)
/*++

Routine Description:

  Validates property parameters.

Arguments:

  PropertyRequest - 
  cbValueSize -
  cbInstanceSize -

Return Value:

  NT status code.

--*/
{
    PAGED_CODE();

    NTSTATUS ntStatus = STATUS_UNSUCCESSFUL;

    if (PropertyRequest && cbValueSize)
    {
        // If the caller is asking for ValueSize.
        //
        if (0 == PropertyRequest->ValueSize) 
        {
            PropertyRequest->ValueSize = cbValueSize;
            ntStatus = STATUS_BUFFER_OVERFLOW;
        }
        // If the caller passed an invalid ValueSize.
        //
        else if (PropertyRequest->ValueSize < cbValueSize)
        {
            ntStatus = STATUS_BUFFER_TOO_SMALL;
        }
        else if (PropertyRequest->InstanceSize < cbInstanceSize)
        {
            ntStatus = STATUS_BUFFER_TOO_SMALL;
        }
        // If all parameters are OK.
        // 
        else if (PropertyRequest->ValueSize >= cbValueSize)
        {
            if (PropertyRequest->Value)
            {
                ntStatus = STATUS_SUCCESS;
                //
                // Caller should set ValueSize, if the property 
                // call is successful.
                //
            }
        }
    }
    else
    {
        ntStatus = STATUS_INVALID_PARAMETER;
    }
    
    // Clear the ValueSize if unsuccessful.
    //
    if (PropertyRequest &&
        STATUS_SUCCESS != ntStatus &&
        STATUS_BUFFER_OVERFLOW != ntStatus)
    {
        PropertyRequest->ValueSize = 0;
    }

    return ntStatus;
} // ValidatePropertyParams

//-----------------------------------------------------------------------------
#pragma code_seg("PAGE")
NTSTATUS
SimpleAudioSamplePropertyDispatch
(
    _In_ PPCPROPERTY_REQUEST PropertyRequest
)
/*++
    Handles and dispatches a SIMPLEAUDIOSAMPLE_PROPERTY_ITEM.

    Use this as the property handler only if the property item is a
    SIMPLEAUDIOSAMPLE_PROPERTY_ITEM.
--*/
{
    PAGED_CODE();

    SIMPLEAUDIOSAMPLE_PROPERTY_ITEM* item = (SIMPLEAUDIOSAMPLE_PROPERTY_ITEM*)PropertyRequest->PropertyItem;

    if (PropertyRequest->Verb & KSPROPERTY_TYPE_BASICSUPPORT)
    {
        if (item->SupportHandler != nullptr)
        {
            return item->SupportHandler(PropertyRequest);
        }
        else
        {
            return PropertyHandler_BasicSupport(PropertyRequest, PropertyRequest->PropertyItem->Flags, VT_ILLEGAL);
        }
    }

    // Verify instance data size
    if (PropertyRequest->InstanceSize < item->MinProperty)
    {
        return STATUS_INVALID_PARAMETER;
    }

    ULONG cbMinSize = item->MinData;

    // Verify value size
    if (PropertyRequest->ValueSize == 0)
    {
        PropertyRequest->ValueSize = cbMinSize;
        return STATUS_BUFFER_OVERFLOW;
    }
    if (PropertyRequest->ValueSize < cbMinSize)
    {
        PropertyRequest->ValueSize = 0;
        return STATUS_BUFFER_TOO_SMALL;
    }

    if (PropertyRequest->Verb & KSPROPERTY_TYPE_GET)
    {
        if (item->GetHandler != nullptr)
        {
            return item->GetHandler(PropertyRequest);
        }
        else
        {
            return STATUS_NOT_SUPPORTED;
        }
    }

    if (PropertyRequest->Verb & KSPROPERTY_TYPE_SET)
    {
        if (item->SetHandler != nullptr)
        {
            return item->SetHandler(PropertyRequest);
        }
        else
        {
            return STATUS_NOT_SUPPORTED;
        }
    }

    return STATUS_INVALID_DEVICE_REQUEST;
}

//-----------------------------------------------------------------------------
#pragma code_seg("PAGE")
NTSTATUS                        
PropertyHandler_BasicSupport
(
    _In_ PPCPROPERTY_REQUEST         PropertyRequest,
    _In_ ULONG                       Flags,
    _In_ DWORD                       PropTypeSetId
)
/*++

Routine Description:

  Default basic support handler. Basic processing depends on the size of data.
  For ULONG it only returns Flags. For KSPROPERTY_DESCRIPTION, the structure   
  is filled.

Arguments:

  PropertyRequest - 

  Flags - Support flags.

  PropTypeSetId - PropTypeSetId

Return Value:
    
    NT status code.

--*/
{
    PAGED_CODE();

    ASSERT(Flags & KSPROPERTY_TYPE_BASICSUPPORT);

    NTSTATUS                    ntStatus = STATUS_INVALID_PARAMETER;

    if (PropertyRequest->ValueSize >= sizeof(KSPROPERTY_DESCRIPTION))
    {
        // if return buffer can hold a KSPROPERTY_DESCRIPTION, return it
        //
        PKSPROPERTY_DESCRIPTION PropDesc = 
            PKSPROPERTY_DESCRIPTION(PropertyRequest->Value);

        PropDesc->AccessFlags       = Flags;
        PropDesc->DescriptionSize   = sizeof(KSPROPERTY_DESCRIPTION);
        if  (VT_ILLEGAL != PropTypeSetId)
        {
            PropDesc->PropTypeSet.Set   = KSPROPTYPESETID_General;
            PropDesc->PropTypeSet.Id    = PropTypeSetId;
        }
        else
        {
            PropDesc->PropTypeSet.Set   = GUID_NULL;
            PropDesc->PropTypeSet.Id    = 0;
        }
        PropDesc->PropTypeSet.Flags = 0;
        PropDesc->MembersListCount  = 0;
        PropDesc->Reserved          = 0;

        PropertyRequest->ValueSize = sizeof(KSPROPERTY_DESCRIPTION);
        ntStatus = STATUS_SUCCESS;
    } 
    else if (PropertyRequest->ValueSize >= sizeof(ULONG))
    {
        // if return buffer can hold a ULONG, return the access flags
        //
        *(PULONG(PropertyRequest->Value)) = Flags;

        PropertyRequest->ValueSize = sizeof(ULONG);
        ntStatus = STATUS_SUCCESS;                    
    }
    else
    {
        PropertyRequest->ValueSize = 0;
        ntStatus = STATUS_BUFFER_TOO_SMALL;
    }

    return ntStatus;
} // PropertyHandler_BasicSupport

