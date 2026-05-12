@ECHO off
SETLOCAL

REM Stable Windows launcher for DeepSeek TUI.
REM It forces a known-good MSVC environment before launching DeepSeek.
REM Preference order:
REM   1. locally built repo binary (target\release\deepseek.exe)
REM   2. npm-installed wrapper

SET "RUSTUP_DIST_SERVER=https://rsproxy.cn"
SET "RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup"
SET "RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc"
SET "CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse"
SET "REPO_DIR=%~dp0"

IF EXIST "%USERPROFILE%\.cargo\gitconfig-no-proxy" (
  SET "GIT_CONFIG_GLOBAL=%USERPROFILE%\.cargo\gitconfig-no-proxy"
)

SET "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
IF NOT EXIST "%VSWHERE%" (
  ECHO [start-deepseek-msvc] vswhere.exe was not found.
  ECHO Install Visual Studio 2022 Build Tools or Community with C++ tools.
  EXIT /B 1
)

SET "VSINSTALL="
FOR /F "usebackq delims=" %%I IN (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) DO SET "VSINSTALL=%%I"
IF NOT DEFINED VSINSTALL (
  ECHO [start-deepseek-msvc] No Visual Studio installation with x64 C++ tools was found.
  EXIT /B 1
)

SET "VSDEV=%VSINSTALL%\Common7\Tools\VsDevCmd.bat"
IF EXIST "%VSDEV%" (
  CALL "%VSDEV%" -no_logo -arch=x64 -host_arch=x64 >NUL
) ELSE (
  SET "VCVARS=%VSINSTALL%\VC\Auxiliary\Build\vcvars64.bat"
  IF EXIST "%VCVARS%" (
    CALL "%VCVARS%" >NUL
  ) ELSE (
    ECHO [start-deepseek-msvc] Neither VsDevCmd.bat nor vcvars64.bat was found.
    EXIT /B 1
  )
)

IF EXIST "D:\bin" (
  SET "PATH=D:\bin;%PATH%"
)
IF EXIST "%REPO_DIR%target\release" (
  SET "PATH=%REPO_DIR%target\release;%PATH%"
)
IF EXIST "%USERPROFILE%\.cargo\bin" (
  SET "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
)
IF EXIST "%APPDATA%\npm" (
  SET "PATH=%APPDATA%\npm;%PATH%"
)

IF EXIST "%REPO_DIR%target\release\deepseek.exe" (
  "%REPO_DIR%target\release\deepseek.exe" %*
  EXIT /B %ERRORLEVEL%
)

IF EXIST "%APPDATA%\npm\deepseek.cmd" (
  CALL "%APPDATA%\npm\deepseek.cmd" %*
  EXIT /B %ERRORLEVEL%
)

WHERE deepseek >NUL 2>NUL
IF %ERRORLEVEL% EQU 0 (
  CALL deepseek %*
  EXIT /B %ERRORLEVEL%
)

ECHO [start-deepseek-msvc] deepseek was not found.
ECHO Install it first with: npm install -g deepseek-tui
EXIT /B 1
