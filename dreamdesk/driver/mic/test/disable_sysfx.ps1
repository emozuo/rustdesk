# Disable system FX (Voice Clarity / AGC / NS APO chain) on the DreamDesk
# Microphone capture endpoint, by setting PKEY_AudioEndpoint_Disable_SysFx = 2.
# Strategy: PowerShell can't directly write here even after take-ownership
# because the ACL on the FxProperties subkey is restrictive. Workaround: use
# psexec -s to run reg.exe as Local System (which has higher privileges than
# Administrators on these endpoint subkeys).

$endpoint = '{bc86f854-5513-453e-8b53-ec020f03cc4d}'
$keyFullPath = "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\$endpoint\FxProperties"
$valueName = '{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5'

# ---- Take ownership + grant Administrators FullControl ----
$keyPathFromHKLM = "SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\$endpoint"
function Take-RegKeyOwnership {
    param([string]$KeyPath)
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey(
        $KeyPath, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree,
        [System.Security.AccessControl.RegistryRights]::TakeOwnership)
    if ($null -eq $key) { return $false }
    $acl = $key.GetAccessControl()
    $admins = New-Object System.Security.Principal.NTAccount("BUILTIN\Administrators")
    $acl.SetOwner($admins)
    $key.SetAccessControl($acl)
    $key.Close()

    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey(
        $KeyPath, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree,
        [System.Security.AccessControl.RegistryRights]::ChangePermissions)
    $acl = $key.GetAccessControl()
    $rule = New-Object System.Security.AccessControl.RegistryAccessRule(
        $admins, "FullControl",
        ([System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
         [System.Security.AccessControl.InheritanceFlags]::ObjectInherit),
        [System.Security.AccessControl.PropagationFlags]::None,
        [System.Security.AccessControl.AccessControlType]::Allow)
    $acl.SetAccessRule($rule)
    $key.SetAccessControl($acl)
    $key.Close()
    return $true
}

Write-Output "Taking ownership of endpoint key (recursive ACL with inherit)..."
[void](Take-RegKeyOwnership $keyPathFromHKLM)

# Also take ownership of FxProperties if it already exists, so its ACL is fresh
[void](Take-RegKeyOwnership "$keyPathFromHKLM\FxProperties")

# Create FxProperties if missing
& reg.exe ADD "$keyFullPath" /f | Out-Null
[void](Take-RegKeyOwnership "$keyPathFromHKLM\FxProperties")

# Set the DWORD value via reg.exe
$out = & reg.exe ADD "$keyFullPath" /v "$valueName" /t REG_DWORD /d 2 /f 2>&1
Write-Output "reg ADD result: $out"

Write-Output "---"
Write-Output "Verifying:"
& reg.exe QUERY "$keyFullPath" /v "$valueName"

Write-Output "---"
Write-Output "Restarting AudioSrv + AudioEndpointBuilder so audio engine re-reads endpoint props..."
Restart-Service -Name AudioEndpointBuilder -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Restart-Service -Name AudioSrv -Force -ErrorAction SilentlyContinue
Write-Output "Done."
