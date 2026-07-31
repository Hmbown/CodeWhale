; codewhale.nsi — NSIS installer for CodeWhale (Windows)
;
; Requirements (see https://github.com/Hmbown/CodeWhale/issues/1983):
;   - Install codewhale.exe, codew.exe, and codewhale-tui.exe side-by-side
;   - Default to %LOCALAPPDATA%\Programs\CodeWhale\bin
;   - Add install dir to current-user PATH
;   - Uninstaller removes the PATH entry
;
; Usage:
;   1. Place all .exe files next to this script:
;        codewhale.exe
;        codew.exe
;        codewhale-tui.exe
;   2. Build:
;        makensis /DVERSION=1.2.3 codewhale.nsi
;   3. Output: CodeWhaleSetup.exe (in current directory)

;--------------------------------
; Includes
;--------------------------------
!include "MUI2.nsh"
!include "FileFunc.nsh"

;--------------------------------
; General
;--------------------------------
!ifndef VERSION
  !define VERSION "0.0.0"
!endif

!define PRODUCT_NAME "CodeWhale"
!define PRODUCT_PUBLISHER "Hmbown"
!define PRODUCT_WEB_SITE "https://github.com/Hmbown/CodeWhale"

Name "${PRODUCT_NAME} ${VERSION}"
OutFile "CodeWhaleSetup.exe"
InstallDir "$LOCALAPPDATA\Programs\CodeWhale"
RequestExecutionLevel user
BrandingText "${PRODUCT_NAME} Installer"

;--------------------------------
; Interface Settings
;--------------------------------
!define MUI_ABORTWARNING
!define MUI_ICON "${NSISDIR}\Contrib\Graphics\Icons\modern-install.ico"
!define MUI_UNICON "${NSISDIR}\Contrib\Graphics\Icons\modern-uninstall.ico"

;--------------------------------
; Pages
;--------------------------------
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

;--------------------------------
; Languages
;--------------------------------
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "SimpChinese"

;--------------------------------
; Installer Sections
;--------------------------------
Section "Install" SecInstall
  SetOutPath "$INSTDIR"
  File "update-user-path.ps1"

  SetOutPath "$INSTDIR\bin"

  ; Copy binaries
  File "codewhale.exe"
  File "codew.exe"
  File "codewhale-tui.exe"

  ; Write uninstaller
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; NSIS strings default to 1024 characters. ReadRegStr returns an empty string
  ; when a registry value exceeds that limit, which used to make a long user
  ; PATH look absent and overwrite it. Use PowerShell/.NET registry APIs so the
  ; complete raw value and its REG_SZ/REG_EXPAND_SZ kind are preserved.
  Call AddToUserPath

  ; Store install directory for uninstaller
  WriteRegStr HKCU "Software\${PRODUCT_NAME}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "QuietUninstallString" "$\"$INSTDIR\Uninstall.exe$\" /S"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "URLInfoAbout" "${PRODUCT_WEB_SITE}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "DisplayVersion" "${VERSION}"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "NoRepair" 1

  ; Calculate and store installed size
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}" "EstimatedSize" "$0"
SectionEnd

Function AddToUserPath
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\update-user-path.ps1" -Operation Add -Entry "$INSTDIR\bin"' $0
  IntCmp $0 0 add_path_done
    DetailPrint "Failed to add CodeWhale to the current-user PATH (exit code $0)."
    IfSilent +2
      MessageBox MB_ICONSTOP|MB_OK "CodeWhale could not safely update your user PATH. Installation has stopped without replacing the existing PATH."
    SetErrorLevel 1
    Abort

  add_path_done:
    SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
FunctionEnd

;--------------------------------
; Uninstaller Section
;--------------------------------
Section "Uninstall"
  ; Remove only CodeWhale's exact entry before deleting the helper. The helper
  ; handles PATH values longer than NSIS_MAX_STRLEN without truncation.
  Call un.RemoveFromUserPath

  ; Remove binaries
  Delete "$INSTDIR\bin\codewhale.exe"
  Delete "$INSTDIR\bin\codew.exe"
  Delete "$INSTDIR\bin\codewhale-tui.exe"
  Delete "$INSTDIR\update-user-path.ps1"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR\bin"
  RMDir "$INSTDIR"

  ; Remove registry keys
  DeleteRegKey HKCU "Software\${PRODUCT_NAME}"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}"
SectionEnd

Function un.RemoveFromUserPath
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\update-user-path.ps1" -Operation Remove -Entry "$INSTDIR\bin"' $0
  IntCmp $0 0 remove_path_done
    DetailPrint "Failed to remove CodeWhale from the current-user PATH (exit code $0)."
    IfSilent +2
      MessageBox MB_ICONSTOP|MB_OK "CodeWhale could not safely update your user PATH. Uninstallation has stopped without replacing the existing PATH."
    SetErrorLevel 1
    Abort

  remove_path_done:
    SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
FunctionEnd
