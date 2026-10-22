@echo off
:: build.cmd - build + sign dreamdeskmic.sys + dreamdesk_mic.cat.
::
:: This is the empirically-validated flow. The earlier
:: VsDevCmd/msbuild wrapper didn't work over non-interactive shells;
:: invoking MSBuild.exe by full path does.
::
:: Usage:  build.cmd            (Release)
::         build.cmd Debug
::
:: Output: x64\<CFG>\package\ -- dreamdeskmic.sys + dreamdesk_mic.inf + dreamdesk_mic.cat
::
:: Release by default: a Debug kernel driver has live ASSERT()s and bugchecks
:: 0x7E on the first assertion when no kernel debugger is attached. The
:: packaging script refuses Debug drivers, so Debug is the exception, not
:: the default.
::
:: ASCII only: cmd.exe decodes a batch file in the system ANSI codepage before
:: chcp can take effect, and a UTF-8 BOM does not help (it reads the BOM as
:: characters). Verified on .68, 2026-09-12.

setlocal enabledelayedexpansion

set CFG=%1
if "%CFG%"=="" set CFG=Release
set OUT=%~dp0x64\%CFG%

:: PROBE for MSBuild and the WDK tools, never hardcode. This used to pin
:: "Visual Studio\2022\Community" and SDK 10.0.26100.0 -- this dev box has
:: VS 18, that directory does not exist, and the script failed on every run.
:: The vhf-input build script next door had been using vswhere all along:
:: two drivers, two ways, and the broken one was the one nobody ran.
:: Store the BARE path and quote at use -- a stored quoted path inside a
:: for /f backtick makes cmd treat "C:\Program as the command name.
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
set MSBUILD=
:: The -find pattern has no spaces and is deliberately unquoted: inside a
:: for /f backtick, a command that both starts and ends with a quote loses
:: the outer pair and the whole line falls apart ('C:\Program' is not
:: recognized).
for /f "usebackq tokens=*" %%M in (`"%VSWHERE%" -latest -requires Microsoft.Component.MSBuild -find MSBuild\**\Bin\MSBuild.exe`) do set "MSBUILD=%%M"
if not defined MSBUILD (
    echo MSBuild.exe not found. Install Visual Studio + WDK, or set MSBUILD.
    exit /b 1
)

:: Take the NEWEST installed SDK tools (this box has both 26100 and 28000).
set INF2CAT=
for /f "usebackq tokens=*" %%I in (`dir /b /o-n "%ProgramFiles(x86)%\Windows Kits\10\bin\10.*" 2^>nul`) do (
    if not defined INF2CAT if exist "%ProgramFiles(x86)%\Windows Kits\10\bin\%%I\x86\Inf2Cat.exe" (
        set "INF2CAT=%ProgramFiles(x86)%\Windows Kits\10\bin\%%I\x86\Inf2Cat.exe"
        set "SIGNTOOL=%ProgramFiles(x86)%\Windows Kits\10\bin\%%I\x64\signtool.exe"
    )
)
if not defined INF2CAT (
    echo Inf2Cat.exe not found under Windows Kits\10\bin -- incomplete WDK.
    exit /b 1
)

echo.
echo === Building dreamdeskmic.vcxproj %CFG% x64...
:: ApiValidator cannot run on this box: WDK 28000's aitstatic.exe returns 193
:: (not a valid Win32 application). That is a toolchain problem, not a driver
:: problem -- the .sys is already built and signed before that step. Disabled
:: for local builds. Turn it back on for a real release: it checks whether the
:: driver uses non-Universal DDIs.
"%MSBUILD%" "%~dp0dreamdeskmic.vcxproj" /p:Configuration=%CFG% /p:Platform=x64 /p:ApiValidator_Enable=false /v:minimal /nologo
if errorlevel 1 (
    echo MSBuild failed.
    exit /b 1
)

:: MSBuild's WDK target auto-creates a self-signed cert "WDKTestCert
:: <hostname>,<random>" in CurrentUser\My, signs the .sys with it, and
:: drops the cert in CurrentUser\Root + TrustedPublisher. We pick that
:: cert by querying the .sys signature.
echo.
echo === Looking up cert that signed dreamdeskmic.sys...
for /f "tokens=*" %%H in ('powershell -NoProfile -Command "(Get-AuthenticodeSignature '%OUT%\dreamdeskmic.sys').SignerCertificate.Thumbprint"') do set THUMB=%%H
if "%THUMB%"=="" (
    echo Could not determine signing cert thumbprint.
    exit /b 1
)
echo Thumbprint: %THUMB%

echo.
echo === Generating dreamdesk_mic.cat with inf2cat...
:: Give inf2cat a CLEAN directory; do not point it at x64\Debug. That also
:: holds MSBuild's obj\dreamdesk_mic.inf, and inf2cat scans subdirectories --
:: picking that one it cannot resolve the .sys by its relative path and fails
:: with "22.9.1 ... is missing or cannot be decompressed from source media".
:: package\ holds only the .sys and .inf, which is also exactly what gets
:: copied to the target machine.
set "PKG=%OUT%\package"
if exist "%PKG%" rmdir /s /q "%PKG%"
mkdir "%PKG%" >nul 2>&1
copy /Y "%OUT%\dreamdeskmic.sys" "%PKG%\" >nul
:: Take MSBuild's STAMPED inf (x64\<CFG>\dreamdesk_mic.inf), NOT the hand-written
:: source one. stampinf rewrites DriverVer to the build date+time
:: (e.g. 09/16/2026,11.10.29.832); the source keeps a frozen literal.
::
:: This used to copy inf\dreamdesk_mic.inf and that is a silent, total no-op on
:: upgrade: pnputil compares ONLY DriverVer, so every rebuild shipped a different
:: .sys under the same 1.0.11.0 and pnputil answered 259 = "no newer package" and
:: kept the old binary. 2026-09-22 on .32 the store still held an April .sys
:: signed by a different cert, months of driver changes never landed, and nothing
:: reported an error. The usb-vhci driver next door ships the stamped one and does
:: not have this bug -- two drivers, two ways, and the broken one was ours.
copy /Y "%OUT%\dreamdesk_mic.inf" "%PKG%\" >nul
findstr /r /c:"^DriverVer *= *[0-9][0-9]/[0-9][0-9]/[0-9][0-9][0-9][0-9]," "%PKG%\dreamdesk_mic.inf" >nul
if errorlevel 1 (
    echo DriverVer in %PKG%\dreamdesk_mic.inf is not stamped -- refusing to package.
    echo stampinf did not run; a same-version install is a silent no-op.
    exit /b 1
)
"%INF2CAT%" /driver:"%PKG%" /os:10_X64
if errorlevel 1 (
    echo inf2cat failed.
    exit /b 1
)

echo.
echo === Signing dreamdesk_mic.cat with %THUMB%...
"%SIGNTOOL%" sign /v /sha1 %THUMB% /fd sha256 "%PKG%\dreamdesk_mic.cat"
if errorlevel 1 (
    echo Catalog signing failed.
    exit /b 1
)

echo.
echo Built and signed. Package in %PKG% (.sys + .inf + .cat)
echo Install:    install.cmd
echo Uninstall:  install.cmd uninstall
