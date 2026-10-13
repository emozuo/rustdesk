# DreamDesk Windows 打包：编出 dreamdesk-pro.exe 和 dreamdesk-pro-<版本>-install.exe。
#
# 复用 RustDesk 的 build.py 而不改它：功能列表由 build.py --print-features 给出（不手抄，免得和它漂移），
# 再加上我们的 dd-pro、dd-brand；Flutter 部分让 build.py 编，跳过它自己的 cargo 和安装包打包两步，
# 改名和打安装包在这里做。
#
# 前置：VS、Flutter 3.24.5、Python 3、Rust、vcpkg（见仓库根的 BUILD-DEPLOY.md）。
# 用法：powershell -File dreamdesk\scripts\build-windows.ps1 [-BuildPyFlags '--hwcodec','--vram']
param(
  [string[]] $BuildPyFlags = @()
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

$Version = ((Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value)
$BaseFeatures = (python3 build.py --flutter @BuildPyFlags --print-features | Select-Object -Last 1).Trim()
if ($LASTEXITCODE -ne 0 -or -not $BaseFeatures) { throw "build.py --print-features failed" }
$Features = "$BaseFeatures,dd-pro,dd-brand"
Write-Host "version $Version, features $Features"

try {
  # pub get 必须在 codegen 之前：codegen 的 ffigen 要读 pub get 生成的 package_config.json
  Invoke-Checked 'flutter pub get' { Push-Location flutter; flutter pub get; Pop-Location }
  Invoke-Checked 'bridge codegen' {
    flutter_rust_bridge_codegen --rust-input ./src/flutter_ffi.rs `
      --dart-output ./flutter/lib/generated_bridge.dart `
      --c-output ./flutter/macos/Runner/bridge_generated.h
  }
  Invoke-Checked 'cargo build (librustdesk)' { cargo build --locked --features $Features --lib --release }
  Invoke-Checked 'flutter build (build.py)' { python3 build.py --flutter @BuildPyFlags --skip-cargo --skip-portable-pack }

  $Release = Join-Path $Repo 'flutter\build\windows\x64\runner\Release'
  $Exe = Join-Path $Release "$AppName.exe"
  Remove-Item $Exe -ErrorAction SilentlyContinue
  Move-Item (Join-Path $Release 'rustdesk.exe') $Exe -ErrorAction Stop

  # 安装包解压后先运行内嵌的程序，所以内嵌的必须已经是改过名的那个
  Push-Location libs\portable -ErrorAction Stop
  try {
    Invoke-Checked 'pip install (portable)' { pip3 install -q -r requirements.txt }
    # generate.py 写好内嵌数据后会顺手编一次打包器，但不带我们的开关；数据不变，带开关再编一次
    Invoke-Checked 'portable generate' { python3 ./generate.py -f $Release -o . -e $Exe }
    Invoke-Checked 'portable packer (dd-brand)' { cargo build --locked --release --features dd-brand }
  } finally { Pop-Location }
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
