@echo off
:: install.cmd - install (or reinstall) the dreamdeskmic driver on this
:: Windows host. Requires test signing already enabled
:: (bcdedit /set testsigning on then reboot) and admin rights.
::
:: Usage:  install.cmd                  (install/upgrade, Release)
::         install.cmd install Debug    (install the Debug build)
::         install.cmd uninstall        (remove)

setlocal enabledelayedexpansion

:: Release by default. A Debug kernel driver has live ASSERT()s -- the first
:: assertion executes a breakpoint, and with no kernel debugger attached that
:: is an immediate 0x7E bugcheck (hit for real on 2026-09-11). The packaging
:: script refuses Debug drivers outright; this path must not be the loophole.
set CFG=Release
if not "%2"=="" set CFG=%2
:: ASCII only: cmd.exe decodes a batch file in the system ANSI codepage before
:: chcp can take effect, and a UTF-8 BOM does not help (it reads the BOM as
:: characters). Verified on .68, 2026-09-12.
::
:: build.cmd puts .sys + .inf + .cat under package\ -- only those three live
:: there, because inf2cat scans subdirectories too and pointing it at
:: x64\Debug would pick the inf under obj\.
set "DRV=%~dp0x64\%CFG%\package"
set "INF=%DRV%\dreamdesk_mic.inf"
set HWID=ROOT\dreamdeskmic

:: Take devcon from the NEWEST installed SDK, never a hardcoded version --
:: this box has both 26100 and 28000, so either constant breaks elsewhere.
set DEVCON=
for /f "usebackq tokens=*" %%T in (`dir /b /o-n "%ProgramFiles(x86)%\Windows Kits\10\Tools\10.*" 2^>nul`) do (
    if not defined DEVCON if exist "%ProgramFiles(x86)%\Windows Kits\10\Tools\%%T\x64\devcon.exe" (
        set "DEVCON=%ProgramFiles(x86)%\Windows Kits\10\Tools\%%T\x64\devcon.exe"
    )
)

if not defined DEVCON (
    echo devcon.exe not found. It ships in the WDK under Tools\.
    exit /b 1
)

if "%1"=="uninstall" goto :uninstall

if not exist "%INF%" (
    echo INF not built yet. Run build.cmd first.
    exit /b 1
)

echo === pnputil add-driver + install...
pnputil /add-driver "%INF%" /install
if errorlevel 1 (
    echo pnputil failed.
    exit /b 1
)

rem `devcon install` is NOT idempotent -- it CREATES A NEW node every run, so N
rem runs leave N nodes and only the first one starts. `remove` takes the hardware
rem id and drops every match, keeping the count at 1. Failing here on a first
rem install is expected (nothing to remove).
rem
rem This still leaves one dead audio endpoint per run under MMDevices\Audio\
rem Capture -- Windows never reaps NOTPRESENT endpoints. Accepted HERE because
rem this is the hand-run developer tool and "reinstall" is its whole point (see
rem the header). The machine-setup script does the opposite: packaging\windows\
rem drivers-install.cmd leaves a healthy node completely alone rather than
rem recreating it, because install-testsigning.ps1 calls it on every setup.
rem (Neither runs during an auto-upgrade -- that path only runs Setup.exe.)
echo === devcon remove + install (recreates ROOT\dreamdeskmic node)...
"%DEVCON%" remove %HWID% >nul 2>&1
"%DEVCON%" install "%INF%" %HWID%
if errorlevel 1 (
    echo devcon failed.
    exit /b 1
)

echo.
echo Done. Speaker and microphone "(DreamDesk Virtual Audio)" should now appear in
echo   Settings > System > Sound (Output and Input)
goto :eof

:uninstall
"%DEVCON%" remove %HWID%
pnputil /delete-driver dreamdesk_mic.inf /uninstall /force
goto :eof
