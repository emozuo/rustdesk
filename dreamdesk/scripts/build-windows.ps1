# DreamDesk Windows 打包：编出 dreamdesk-pro.exe 和 dreamdesk-pro-<版本>-install.exe。
#
# 编译交给 RustDesk 的 build.py（它的 --dd-pro、--dd-brand 开关映射成编译开关，和别的开关同一处），
# 跳过它自己的安装包打包；改名和打安装包在这里做（安装包里的程序必须已经是改过名的）。
#
# 前置：VS、Flutter 3.24.5、Python 3、Rust、vcpkg（见仓库根的 BUILD-DEPLOY.md）。
# 用法：powershell -File dreamdesk\scripts\build-windows.ps1
# 默认 --hwcodec --vram：和官方 Windows x64 发布版的构建参数一致（.github/workflows/flutter-build.yml），
# 通用模式因此和官方原版一样；专业模式的采集要用 vram 带来的「直接输出显卡纹理」。
param(
  [string[]] $BuildPyFlags = @('--hwcodec', '--vram')
)
# 外部程序（cargo、flutter、python）把进度写到 stderr，PowerShell 5.1 在 Stop 下会把它当致命错误；
# 所以全局用 Continue，外部程序只看退出码（Invoke-Checked），文件操作各自 -ErrorAction Stop。
$ErrorActionPreference = 'Continue'

# 与 src/ddpro/brand.rs 的 APP_NAME 保持一致：安装时 RustDesk 按应用名命名程序，两边不一致会装出两个名字
$AppName = 'dreamdesk-pro'

$Repo = (Resolve-Path "$PSScriptRoot\..\.." -ErrorAction Stop).Path
Set-Location $Repo -ErrorAction Stop
if (-not (Select-String -Path src\ddpro\brand.rs -SimpleMatch "pub const APP_NAME: &str = `"$AppName`";" -Quiet)) {
  throw "AppName=$AppName does not match APP_NAME in src\ddpro\brand.rs"
}

function Invoke-Checked([string] $What, [scriptblock] $Block) {
  Write-Host "########## $What ##########"
  # 命令根本不存在时 PowerShell 只记一条错误、不动退出码，留着上一条命令的 0 就会被当成成功：先放一个哨兵值
  $global:LASTEXITCODE = -1
  # 2>&1 后取原始文本：外部程序的 stderr 原样显示，不被 PowerShell 包成 NativeCommandError（空行也不会变成 RemoteException）；
  # PowerShell 自己的错误（找不到命令等）照常显示错误信息
  & $Block 2>&1 | ForEach-Object {
    if ($_ -is [System.Management.Automation.ErrorRecord]) {
      if ($_.Exception -is [System.Management.Automation.RemoteException]) { "$($_.TargetObject)" } else { "$_" }
    } else { "$_" }
  }
  if ($LASTEXITCODE -ne 0) { throw "$What failed (exit $LASTEXITCODE)" }
}

# 官方 Windows 发布流程（.github/workflows/flutter-build.yml 的 build-for-windows-flutter，和它依赖的
# third-party-RustDeskTempTopMostWindow.yml）在 build.py 之外还往程序目录里放了三样，本地照做：
# 隐私模式要加载的 WindowInjection.dll、虚拟显示器驱动 usbmmidd_v2、远程打印驱动。
# 地址、版本、编译参数都从这两个文件里读，不手抄，上游改了跟着变；读不到就报错。
# 签名（要官方证书）和 MSI（另一种安装包）不做。下载要能连 GitHub（本机走 ALL_PROXY）。
$Deps = Join-Path $Repo 'target\dreamdesk-deps'
$Workflow = '.github\workflows\flutter-build.yml'
$TopMostWorkflow = '.github\workflows\third-party-RustDeskTempTopMostWindow.yml'

function Get-WorkflowValue([string] $File, [string] $Pattern) {
  $m = Select-String -Path (Join-Path $Repo $File) -Pattern $Pattern | Select-Object -First 1
  if (-not $m) { throw "cannot find '$Pattern' in $File" }
  $m.Matches[0].Groups[1].Value
}

function Get-Download([string] $Url, [switch] $NoCache) {
  $Out = Join-Path $Deps (Split-Path $Url -Leaf)
  if ($NoCache -or -not (Test-Path $Out)) {
    # 先下到 .part 成功再改名：失败留下的半截文件不能进缓存被下次当成好的
    $Part = "$Out.part"
    try {
      # Out-Host：命令输出会混进函数返回值，这里只能返回路径
      Invoke-Checked "download $Url" { curl.exe -fsSL --retry 3 -o $Part $Url } | Out-Host
    } catch {
      Remove-Item $Part -Force -ErrorAction SilentlyContinue
      throw
    }
    Move-Item $Part $Out -Force -ErrorAction Stop
  }
  $Out
}

# 下载、clone、编译都在 build.py 之前做：缺代理或缺 C++ 工作负载时立刻报错，不用等完整编译完
function Get-OfficialPayload {
  New-Item -ItemType Directory -Force $Deps | Out-Null

  # WindowInjection.dll：隐私模式（截屏时排除、放大镜两种实现）要把它注入一个辅助进程，没有它直接报失败
  $Url = Get-WorkflowValue $TopMostWorkflow 'git clone (\S+)'
  $Commit = Get-WorkflowValue $TopMostWorkflow 'git checkout ([0-9a-f]{40})'
  $Project = Get-WorkflowValue $TopMostWorkflow 'project_path:\s*(\S+)'
  $Src = Join-Path $Deps 'RustDeskTempTopMostWindow'
  if (-not (Test-Path $Src)) { Invoke-Checked 'clone RustDeskTempTopMostWindow' { git clone -q $Url $Src } | Out-Host }
  git -C $Src checkout -q $Commit 2>$null
  if ($LASTEXITCODE -ne 0) { Invoke-Checked "checkout $Commit" { git -C $Src fetch -q origin; git -C $Src checkout -q $Commit } | Out-Host }
  $MsBuild = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -requires Microsoft.Component.MSBuild -find 'MSBuild\**\Bin\MSBuild.exe' | Select-Object -First 1
  if (-not $MsBuild) { throw 'MSBuild not found (Visual Studio with the C++ workload is required)' }
  # 和官方调用一致：Release、x64、TargetVersion=Windows10（flutter-build.yml 的 build-RustDeskTempTopMostWindow）。
  # 工程指定 VS 2019 的工具集；本机没装就用已装的、不低于它的最老的那个（Flutter 外壳和插件本来也是用本机工具集编的），日志里写明
  $ProjFile = Join-Path $Src $Project
  $Want = (Select-String -Path $ProjFile -Pattern '<PlatformToolset>(v\d+)</PlatformToolset>' | Select-Object -First 1).Matches.Groups[1].Value
  $VcRoot = Resolve-Path (Join-Path (Split-Path $MsBuild) '..\..\Microsoft\VC') -ErrorAction Stop
  $Have = Get-ChildItem "$VcRoot\*\Platforms\x64\PlatformToolsets\*" -Directory |
    ForEach-Object Name | Where-Object { $_ -match '^v\d+$' } | Sort-Object { [int]$_.Substring(1) } -Unique
  $Toolset = @()
  if ($Want -and $Have -notcontains $Want) {
    $Use = $Have | Where-Object { [int]$_.Substring(1) -ge [int]$Want.Substring(1) } | Select-Object -First 1
    if (-not $Use) {
      $Use = $Have | Select-Object -Last 1
      Write-Warning "no installed toolset is at least $Want; falling back to the older $Use"
    }
    Write-Warning "WindowInjection asks for toolset $Want, not installed; building with $Use"
    $Toolset = @("-p:PlatformToolset=$Use")
  }
  Invoke-Checked 'msbuild WindowInjection' {
    & $MsBuild $ProjFile -nologo -v:minimal -p:Configuration=Release -p:Platform=x64 /p:TargetVersion=Windows10 @Toolset
  } | Out-Host

  # 虚拟显示器驱动：「隐私模式 2」（虚拟显示器实现）和虚拟显示器功能要装它
  $Zip = Get-Download (Get-WorkflowValue $Workflow 'Invoke-WebRequest -Uri (\S+/usbmmidd_v2\.zip)')

  # 远程打印驱动：和官方一样按发布页的 sha256sums 校验。官方对不上就跳过，这里报错：缺了它就不是完整的官方包。
  # sha256sums 挂在固定的 release tag 下、文件名不带版本，缓存的旧文件查不到官方升级后新 zip 的校验值，所以每次重下
  $DriverZip = Get-Download (Get-WorkflowValue $Workflow 'Invoke-WebRequest -Uri (\S+/rustdesk_printer_driver_v[\d.-]+\.zip)')
  $AdapterZip = Get-Download (Get-WorkflowValue $Workflow 'Invoke-WebRequest -Uri (\S+/printer_driver_adapter\.zip)')
  $Sums = Get-Download (Get-WorkflowValue $Workflow 'Invoke-WebRequest -Uri (\S+/driver/sha256sums)') -NoCache
  foreach ($F in $DriverZip, $AdapterZip) {
    $Name = [regex]::Escape((Split-Path $F -Leaf))
    $Want = (Select-String -Path $Sums -Pattern "^([a-fA-F0-9]{64}) \*$Name$").Matches.Groups[1].Value
    if ($Want -ne (Get-FileHash $F -Algorithm SHA256).Hash) {
      # 同名文件官方可能换过，删掉缓存，重跑会重新下载
      Remove-Item $F -Force -ErrorAction SilentlyContinue
      throw "$F checksum mismatch against $Sums (cached copy deleted, rerun to download again)"
    }
  }

  @{
    Dll        = Join-Path $Src 'WindowInjection\x64\Release\WindowInjection.dll'
    UsbZip     = $Zip
    DriverZip  = $DriverZip
    AdapterZip = $AdapterZip
  }
}

function Add-OfficialPayload([string] $Dest, [hashtable] $Payload) {
  Copy-Item $Payload.Dll $Dest -Force -ErrorAction Stop

  # 虚拟显示器驱动：官方删掉 32 位目录和安装器程序
  $Zip = $Payload.UsbZip
  $Tmp = Join-Path $Deps 'usbmmidd-extract'
  Remove-Item $Tmp -Recurse -Force -ErrorAction SilentlyContinue
  Expand-Archive $Zip -DestinationPath $Tmp -ErrorAction Stop
  Remove-Item "$Tmp\usbmmidd_v2\Win32" -Recurse -ErrorAction Stop
  Remove-Item "$Tmp\usbmmidd_v2\deviceinstaller64.exe", "$Tmp\usbmmidd_v2\deviceinstaller.exe", "$Tmp\usbmmidd_v2\usbmmidd.bat" -ErrorAction Stop
  Remove-Item (Join-Path $Dest 'usbmmidd_v2') -Recurse -Force -ErrorAction SilentlyContinue
  Move-Item "$Tmp\usbmmidd_v2" $Dest -ErrorAction Stop

  # 远程打印驱动（Get-OfficialPayload 已校验）；先删上次放进去的，这次中途失败时不会留着旧驱动
  $DriverName = [IO.Path]::GetFileNameWithoutExtension($Payload.DriverZip)
  Remove-Item (Join-Path $Dest 'drivers') -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item (Join-Path $Dest 'printer_driver_adapter.dll') -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force (Join-Path $Dest 'drivers') | Out-Null
  Expand-Archive $Payload.DriverZip -DestinationPath $Tmp -Force -ErrorAction Stop
  Move-Item (Join-Path $Tmp $DriverName) (Join-Path $Dest 'drivers\RustDeskPrinterDriver') -ErrorAction Stop
  Expand-Archive $Payload.AdapterZip -DestinationPath $Tmp -Force -ErrorAction Stop
  Move-Item (Join-Path $Tmp 'printer_driver_adapter.dll') $Dest -Force -ErrorAction Stop
  Remove-Item $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}

# USB 透传的被控端驱动 usbip-win2（P2 第 1 项）：官方安装程序原样放进程序目录的 usbip\ 下，产品安装时 `--ddpro-usbip install`
# 静默装上（版本从文件名来）。锁版本 + sha256，并核对微软签名链上的发布者签名有效。0.9.7.8 有蓝屏，别回退到它；
# 锁 0.9.8.0：P2 的实验和压测都在它上面跑。要升 0.9.8.1（sha256 38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a）
# 得把透传压测重跑一遍。
$UsbipVer = '0.9.8.0'
$UsbipSha = '81f426741f7ee2ed991febe24a22daca8400b6ae2f171054e3fb404897e15d39'

function Get-UsbipInstaller {
  $File = Get-Download "https://github.com/vadimgrn/usbip-win2/releases/download/v.$UsbipVer/USBip-$UsbipVer-x64.exe"
  if ((Get-FileHash $File -Algorithm SHA256).Hash -ne $UsbipSha) {
    Remove-Item $File -Force -ErrorAction SilentlyContinue
    throw "$File checksum mismatch (cached copy deleted, rerun to download again)"
  }
  if ((Get-AuthenticodeSignature $File).Status -ne 'Valid') { throw "$File is not validly signed" }
  $File
}

function Add-UsbipInstaller([string] $Dest, [string] $File) {
  $Dir = Join-Path $Dest 'usbip'
  Remove-Item $Dir -Recurse -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force $Dir | Out-Null
  Copy-Item $File $Dir -ErrorAction Stop
}

$Version = ((Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value)
$BuildPyFlags += @('--dd-pro', '--dd-brand')
Write-Host "version $Version"
$Payload = Get-OfficialPayload
$UsbipInstaller = Get-UsbipInstaller

# flutter pub get 会改写这个被跟踪的文件：打完用改前的原字节还原（不用 git checkout，那样会丢掉本地未提交的改动）
$PubLock = Join-Path $Repo 'flutter\pubspec.lock'
$PubLockBytes = [IO.File]::ReadAllBytes($PubLock)
try {
  # pub get 必须在 codegen 之前：codegen 的 ffigen 要读 pub get 生成的 package_config.json
  Invoke-Checked 'flutter pub get' { Push-Location flutter; flutter pub get; Pop-Location }
  Invoke-Checked 'bridge codegen' {
    flutter_rust_bridge_codegen --rust-input ./src/flutter_ffi.rs `
      --dart-output ./flutter/lib/generated_bridge.dart `
      --c-output ./flutter/macos/Runner/bridge_generated.h
  }
  Invoke-Checked 'cargo + flutter build (build.py)' { python3 build.py --flutter @BuildPyFlags --skip-portable-pack }

  $Release = Join-Path $Repo 'flutter\build\windows\x64\runner\Release'
  Add-OfficialPayload $Release $Payload
  Add-UsbipInstaller $Release $UsbipInstaller
  $Exe = Join-Path $Release "$AppName.exe"
  Remove-Item $Exe -ErrorAction SilentlyContinue
  Move-Item (Join-Path $Release 'rustdesk.exe') $Exe -ErrorAction Stop

  # 自检：产物里不许再有带 rustdesk 字样的可执行文件（librustdesk.dll 是被加载的库，不是进程，不在此列）。
  # 放在打安装包之前：查出问题时不能已经把上一个好的安装包覆盖掉
  $Left = Get-ChildItem $Release -Recurse -Filter '*.exe' | Where-Object { $_.Name -match 'rustdesk' }
  if ($Left) { throw "rustdesk-named executables left in the bundle: $($Left.FullName -join ', ')" }

  # 安装包解压后先运行内嵌的程序，所以内嵌的必须已经是改过名的那个
  $Manifest = Join-Path $Repo 'res\manifest.xml'
  $ManifestBytes = $null
  Push-Location libs\portable -ErrorAction Stop
  try {
    Invoke-Checked 'pip install (portable)' { pip3 install -q -r requirements.txt }
    # 官方打安装器前删掉清单里的 dpiAware 一行（flutter-build.yml「Build self-extracted executable」），打完还原
    # 和 sed '/dpiAware/d' 一样，下一行 dpiAwareness 也含这个词，一并删掉；原文件无 BOM，写回也不带
    # 还原用改前存下的原字节，不用 git checkout：那样会丢掉本地未提交的清单改动
    $ManifestBytes = [IO.File]::ReadAllBytes($Manifest)
    [IO.File]::WriteAllLines($Manifest, [string[]]((Get-Content $Manifest) | Where-Object { $_ -notmatch 'dpiAware' }))
    # generate.py 写好内嵌数据后会顺手编一次打包器，但不带我们的开关；数据不变，带开关再编一次
    Invoke-Checked 'portable generate' { python3 ./generate.py -f $Release -o . -e $Exe }
    Invoke-Checked 'portable packer (dd-brand)' { cargo build --locked --release --features dd-brand }
  } finally {
    Pop-Location
    if ($ManifestBytes) { [IO.File]::WriteAllBytes($Manifest, $ManifestBytes) }
  }
  $Installer = Join-Path $Repo "$AppName-$Version-install.exe"
  Copy-Item (Join-Path $Repo 'target\release\rustdesk-portable-packer.exe') $Installer -Force -ErrorAction Stop

  Write-Host "########## output ##########"
  Get-Item $Exe, $Installer | Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB, 1) } } | Format-Table -AutoSize
} finally {
  [IO.File]::WriteAllBytes($PubLock, $PubLockBytes)
}
