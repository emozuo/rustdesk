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

function Invoke-Checked([string] $What, [scriptblock] $Block) {
  Write-Host "########## $What ##########"
  # 2>&1 后取原始文本：stderr 原样显示，不被 PowerShell 包成 NativeCommandError（空行也不会变成 RemoteException）
  & $Block 2>&1 | ForEach-Object {
    if ($_ -is [System.Management.Automation.ErrorRecord]) { "$($_.TargetObject)" } else { "$_" }
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

function Get-Download([string] $Url) {
  $Out = Join-Path $Deps (Split-Path $Url -Leaf)
  if (-not (Test-Path $Out)) {
    Invoke-Checked "download $Url" { curl.exe -fsSL --retry 3 -o $Out $Url }
  }
  $Out
}

function Add-OfficialPayload([string] $Dest) {
  New-Item -ItemType Directory -Force $Deps | Out-Null

  # WindowInjection.dll：隐私模式（截屏时排除、放大镜两种实现）要把它注入一个辅助进程，没有它直接报失败
  $Url = Get-WorkflowValue $TopMostWorkflow 'git clone (\S+)'
  $Commit = Get-WorkflowValue $TopMostWorkflow 'git checkout ([0-9a-f]{40})'
  $Project = Get-WorkflowValue $TopMostWorkflow 'project_path:\s*(\S+)'
  $Src = Join-Path $Deps 'RustDeskTempTopMostWindow'
  if (-not (Test-Path $Src)) { Invoke-Checked 'clone RustDeskTempTopMostWindow' { git clone -q $Url $Src } }
  git -C $Src checkout -q $Commit 2>$null
  if ($LASTEXITCODE -ne 0) { Invoke-Checked "checkout $Commit" { git -C $Src fetch -q origin; git -C $Src checkout -q $Commit } }
  $MsBuild = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -requires Microsoft.Component.MSBuild -find 'MSBuild\**\Bin\MSBuild.exe' | Select-Object -First 1
  if (-not $MsBuild) { throw 'MSBuild not found (Visual Studio with the C++ workload is required)' }
  # 和官方调用一致：Release、x64、TargetVersion=Windows10（flutter-build.yml 的 build-RustDeskTempTopMostWindow）。
  # 工程指定 VS 2019 的工具集；本机没装就用本机最新的（Flutter 外壳和插件本来也是用本机工具集编的），日志里写明
  $ProjFile = Join-Path $Src $Project
  $Want = (Select-String -Path $ProjFile -Pattern '<PlatformToolset>(v\d+)</PlatformToolset>' | Select-Object -First 1).Matches.Groups[1].Value
  $VcRoot = Resolve-Path (Join-Path (Split-Path $MsBuild) '..\..\Microsoft\VC') -ErrorAction Stop
  $Have = Get-ChildItem "$VcRoot\*\Platforms\x64\PlatformToolsets\*" -Directory |
    ForEach-Object Name | Where-Object { $_ -match '^v\d+$' } | Sort-Object { [int]$_.Substring(1) } -Unique
  $Toolset = @()
  if ($Want -and $Have -notcontains $Want) {
    $Use = $Have | Select-Object -Last 1
    Write-Warning "WindowInjection asks for toolset $Want, not installed; building with $Use"
    $Toolset = @("-p:PlatformToolset=$Use")
  }
  Invoke-Checked 'msbuild WindowInjection' {
    & $MsBuild $ProjFile -nologo -v:minimal -p:Configuration=Release -p:Platform=x64 /p:TargetVersion=Windows10 @Toolset
  }
  Copy-Item (Join-Path $Src 'WindowInjection\x64\Release\WindowInjection.dll') $Dest -Force -ErrorAction Stop

  # 虚拟显示器驱动：「隐私模式 2」（虚拟显示器实现）和虚拟显示器功能要装它；官方删掉 32 位目录和安装器程序
  $Zip = Get-Download (Get-WorkflowValue $Workflow 'Invoke-WebRequest -Uri (\S+/usbmmidd_v2\.zip)')
  $Tmp = Join-Path $Deps 'usbmmidd-extract'
  Remove-Item $Tmp -Recurse -Force -ErrorAction SilentlyContinue
  Expand-Archive $Zip -DestinationPath $Tmp -ErrorAction Stop
  Remove-Item "$Tmp\usbmmidd_v2\Win32" -Recurse -ErrorAction Stop
  Remove-Item "$Tmp\usbmmidd_v2\deviceinstaller64.exe", "$Tmp\usbmmidd_v2\deviceinstaller.exe", "$Tmp\usbmmidd_v2\usbmmidd.bat" -ErrorAction Stop
  Remove-Item (Join-Path $Dest 'usbmmidd_v2') -Recurse -Force -ErrorAction SilentlyContinue
  Move-Item "$Tmp\usbmmidd_v2" $Dest -ErrorAction Stop

  # 远程打印驱动：和官方一样按发布页的 sha256sums 校验，对不上就跳过（官方也是跳过、不让构建失败）
  $DriverZip = Get-Download (Get-WorkflowValue $Workflow 'Invoke-WebRequest -Uri (\S+/rustdesk_printer_driver_v[\d.-]+\.zip)')
  $AdapterZip = Get-Download (Get-WorkflowValue $Workflow 'Invoke-WebRequest -Uri (\S+/printer_driver_adapter\.zip)')
  $Sums = Get-Download (Get-WorkflowValue $Workflow 'Invoke-WebRequest -Uri (\S+/driver/sha256sums)')
  $Ok = $true
  foreach ($F in $DriverZip, $AdapterZip) {
    $Name = [regex]::Escape((Split-Path $F -Leaf))
    $Want = (Select-String -Path $Sums -Pattern "^([a-fA-F0-9]{64}) \*$Name$").Matches.Groups[1].Value
    if ($Want -ne (Get-FileHash $F -Algorithm SHA256).Hash) { Write-Warning "$F checksum mismatch, printer driver skipped"; $Ok = $false }
  }
  if ($Ok) {
    $DriverName = [IO.Path]::GetFileNameWithoutExtension($DriverZip)
    Remove-Item (Join-Path $Dest 'drivers') -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force (Join-Path $Dest 'drivers') | Out-Null
    Expand-Archive $DriverZip -DestinationPath $Tmp -Force -ErrorAction Stop
    Move-Item (Join-Path $Tmp $DriverName) (Join-Path $Dest 'drivers\RustDeskPrinterDriver') -ErrorAction Stop
    Expand-Archive $AdapterZip -DestinationPath $Tmp -Force -ErrorAction Stop
    Move-Item (Join-Path $Tmp 'printer_driver_adapter.dll') $Dest -Force -ErrorAction Stop
  }
  Remove-Item $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}

$Version = ((Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value)
$BuildPyFlags += @('--dd-pro', '--dd-brand')
$Features = (python3 build.py --flutter @BuildPyFlags --print-features | Select-Object -Last 1).Trim()
if ($LASTEXITCODE -ne 0 -or -not $Features) { throw "build.py --print-features failed" }
Write-Host "version $Version, features $Features"

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
  Add-OfficialPayload $Release
  $Exe = Join-Path $Release "$AppName.exe"
  Remove-Item $Exe -ErrorAction SilentlyContinue
  Move-Item (Join-Path $Release 'rustdesk.exe') $Exe -ErrorAction Stop

  # 安装包解压后先运行内嵌的程序，所以内嵌的必须已经是改过名的那个
  Push-Location libs\portable -ErrorAction Stop
  try {
    Invoke-Checked 'pip install (portable)' { pip3 install -q -r requirements.txt }
    # 官方打安装器前删掉清单里的 dpiAware 一行（flutter-build.yml「Build self-extracted executable」），打完还原
    # 和 sed '/dpiAware/d' 一样，下一行 dpiAwareness 也含这个词，一并删掉；原文件无 BOM，写回也不带
    $Manifest = Join-Path $Repo 'res\manifest.xml'
    [IO.File]::WriteAllLines($Manifest, [string[]]((Get-Content $Manifest) | Where-Object { $_ -notmatch 'dpiAware' }))
    # generate.py 写好内嵌数据后会顺手编一次打包器，但不带我们的开关；数据不变，带开关再编一次
    Invoke-Checked 'portable generate' { python3 ./generate.py -f $Release -o . -e $Exe }
    Invoke-Checked 'portable packer (dd-brand)' { cargo build --locked --release --features dd-brand }
  } finally {
    Pop-Location
    git checkout -- res/manifest.xml 2>$null
  }
  $Installer = Join-Path $Repo "$AppName-$Version-install.exe"
  Copy-Item (Join-Path $Repo 'target\release\rustdesk-portable-packer.exe') $Installer -Force -ErrorAction Stop

  # 自检：产物里不许再有带 rustdesk 字样的可执行文件（librustdesk.dll 是被加载的库，不是进程，不在此列）
  $Left = Get-ChildItem $Release -Recurse -Filter '*.exe' | Where-Object { $_.Name -match 'rustdesk' }
  if ($Left) { throw "rustdesk-named executables left in the bundle: $($Left.FullName -join ', ')" }

  Write-Host "########## output ##########"
  Get-Item $Exe, $Installer | Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB, 1) } } | Format-Table -AutoSize
} finally {
  # flutter pub get 会改写这个被跟踪的文件，还原，免得混进提交
  git checkout -- flutter/pubspec.lock 2>$null
}
