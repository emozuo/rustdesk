$base = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture"
Get-ChildItem $base -ErrorAction SilentlyContinue | ForEach-Object {
    $epId = $_.PSChildName
    $propPath = "$($_.PSPath)\Properties"
    $props = Get-ItemProperty $propPath -ErrorAction SilentlyContinue
    if ($props) {
        # Try multiple property keys for the device name
        $name1 = $props."{a45c254e-df1c-4efd-8020-67d146a850e0},2"
        $name2 = $props."{b3f8fa53-0004-438e-9003-51a46e139bfc},6"
        $name3 = $props."{a45c254e-df1c-4efd-8020-67d146a850e0},14"
        $combined = "$name1 | $name2 | $name3"
        if ($combined -like "*DreamDesk*" -or $combined -like "*reamDesk*") {
            Write-Output "endpoint=$epId"
            Write-Output "  desc=$combined"
        }
    }
}
