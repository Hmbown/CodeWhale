; codewhale.nsi — NSIS installer for CodeWhale (Windows)
;
; Requirements (see https://github.com/Hmbown/CodeWhale/issues/1983):
;   - Install codewhale.exe and codewhale-tui.exe side-by-side
;   - Default to %LOCALAPPDATA%\Programs\CodeWhale\bin
;   - Add install dir to current-user PATH
;   - Uninstaller removes the PATH entry
;
; Usage:
;   1. Place both .exe files next to this script:
;        codewhale.exe
;        codewhale-tui.exe
;   2. Build:
;        makensis codewhale.nsi
;   3. Output: CodeWhaleSetup.exe (in current directory)
;
; You can override version at build time:
;   makensis /DVERSION=1.2.3 codewhale.nsi

;--------------------------------
; Includes
;--------------------------------
!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "StrFunc.nsh"

${StrStr}
${UnStrStr}

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
  SetOutPath "$INSTDIR\bin"

  ; Copy binaries
  File "codewhale.exe"
  File "codewhale-tui.exe"

  ; Write uninstaller
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; Add to current-user PATH
  ; Read existing PATH, append if not already present
  ReadRegStr $0 HKCU "Environment" "Path"
  ${StrStr} $1 $0 "$INSTDIR\bin"
  StrCmp $1 "" 0 path_already_set
    ; Not found — append
    StrCmp $0 "" empty_path
      WriteRegExpandStr HKCU "Environment" "Path" "$0;$INSTDIR\bin"
      Goto path_done
    empty_path:
      WriteRegExpandStr HKCU "Environment" "Path" "$INSTDIR\bin"
    path_done:
    ; Notify the system about the environment change
    SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
  path_already_set:

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

;--------------------------------
; Uninstaller Section
;--------------------------------
Section "Uninstall"
  ; Remove binaries
  Delete "$INSTDIR\bin\codewhale.exe"
  Delete "$INSTDIR\bin\codewhale-tui.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR\bin"
  RMDir "$INSTDIR"

  ; Remove from current-user PATH
  ReadRegStr $0 HKCU "Environment" "Path"
  ${un.StrStr} $1 $0 "$INSTDIR\bin"
  StrCmp $1 "" path_clean_done
    ; Remove the entry
    Push "$0"
    Push "$INSTDIR\bin"
    Call un.RemoveFromPath
    Pop $0
    WriteRegExpandStr HKCU "Environment" "Path" "$0"
    SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
  path_clean_done:

  ; Remove registry keys
  DeleteRegKey HKCU "Software\${PRODUCT_NAME}"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}"
SectionEnd

;--------------------------------
; Helper: Remove a directory from PATH (uninstaller version)
; Input: PATH string (on stack), directory to remove (on stack)
; Output: cleaned PATH (on stack)
;--------------------------------
Function un.RemoveFromPath
  Exch $R0 ; directory to remove
  Exch
  Exch $R1 ; original PATH
  Push $R2 ; prefix
  Push $R3 ; suffix
  Push $R4 ; match result

  ${un.StrStr} $R4 $R1 $R0
  StrCmp $R4 "" done

  ; Calculate prefix
  StrLen $R2 $R1
  StrLen $R3 $R4
  IntOp $R3 $R2 - $R3 ; Match offset
  StrCpy $R2 $R1 $R3   ; Prefix string

  ; Calculate suffix
  StrLen $R4 $R0
  IntOp $R4 $R3 + $R4 ; Suffix offset = Match offset + Dir length
  StrCpy $R3 $R1 "" $R4 ; Suffix string

  ; Clean up semicolons
  StrCpy $R4 $R3 1
  StrCmp $R4 ";" 0 +2
    StrCpy $R3 $R3 "" 1 ; Strip leading semicolon from suffix

  StrLen $R4 $R2
  IntOp $R4 $R4 - 1
  StrCpy $R0 $R2 1 $R4
  StrCmp $R0 ";" 0 +2
    StrCpy $R2 $R2 $R4 ; Strip trailing semicolon from prefix

  ; Concatenate
  StrCmp $R2 "" 0 +3
    StrCpy $R1 $R3
    Goto done
  StrCmp $R3 "" 0 +3
    StrCpy $R1 $R2
    Goto done
  StrCpy $R1 "$R2;$R3"

  done:
    Pop $R4
    Pop $R3
    Pop $R2
    Pop $R0
    Exch $R1
FunctionEnd
