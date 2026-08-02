@echo off
setlocal

set "OHOS_LINKER_SCRIPT=%~dp0ohos-clang.ps1"
if not exist "%OHOS_LINKER_SCRIPT%" (
    echo error: OpenHarmony linker script is missing: %OHOS_LINKER_SCRIPT% 1>&2
    exit /b 1
)

rem Re-quote every argument before forwarding: cmd's %* expansion strips the
rem quotes that rustc put around arguments containing spaces, so a sysroot like
rem "D:\DevEco Studio\...\sysroot" would otherwise arrive at clang split on the
rem space. %~1 strips the caller's quoting, then we wrap the argument in quotes
rem again so PowerShell -File hands it to the script as one argument.
set "ARGS="
:argloop
if "%~1"=="" goto run
set "ARGS=%ARGS% "%~1""
shift
goto argloop

:run
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%OHOS_LINKER_SCRIPT%" %ARGS%
exit /b %ERRORLEVEL%
