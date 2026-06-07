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
;        makensis /DVERSION=1.2.3 codewhale.nsi
;   3. Output: CodeWhaleSetup.exe (in current directory)

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
  ; Read existing PATH, append only when the exact entry is absent.
  ReadRegStr $0 HKCU "Environment" "Path"
  StrCpy $2 ";$0;"
  StrCpy $3 ";$INSTDIR\bin;"
  ${StrStr} $1 $2 $3
  StrCmp $1 "" 0 path_already_set
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
  StrCpy $2 ";$0;"
  StrCpy $3 ";$INSTDIR\bin;"
  ${UnStrStr} $1 $2 $3
  StrCmp $1 "" path_clean_done
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
; Helper: Remove exact directory entries from PATH (uninstaller version)
; Input: PATH string (on stack), directory to remove (on stack)
; Output: cleaned PATH (on stack)
;--------------------------------
Function un.RemoveFromPath
  Exch $R0 ; directory to remove
  Exch
  Exch $R1 ; original PATH
  Push $R2 ; padded path
  Push $R3 ; padded needle
  Push $R4 ; match result
  Push $R5 ; prefix
  Push $R6 ; suffix
  Push $R7 ; offset/length

  loop:
    StrCmp $R1 "" done
    StrCpy $R2 ";$R1;"
    StrCpy $R3 ";$R0;"
    ${UnStrStr} $R4 $R2 $R3
    StrCmp $R4 "" done

    ; Prefix before the exact `;dir;` match in the padded PATH.
    StrLen $R5 $R2
    StrLen $R6 $R4
    IntOp $R6 $R5 - $R6
    StrCpy $R5 $R2 $R6

    ; Suffix after the exact `;dir;` match in the padded PATH.
    StrLen $R7 $R3
    IntOp $R7 $R6 + $R7
    StrCpy $R6 $R2 "" $R7

    Push $R5
    Call un.TrimPathEdgeSemicolons
    Pop $R5
    Push $R6
    Call un.TrimPathEdgeSemicolons
    Pop $R6

    StrCmp $R5 "" 0 +3
      StrCpy $R1 $R6
      Goto loop
    StrCmp $R6 "" 0 +3
      StrCpy $R1 $R5
      Goto loop
    StrCpy $R1 "$R5;$R6"
    Goto loop

  done:
    Pop $R7
    Pop $R6
    Pop $R5
    Pop $R4
    Pop $R3
    Pop $R2
    Pop $R0
    Exch $R1
FunctionEnd

Function un.TrimPathEdgeSemicolons
  Exch $R9
  Push $R8

  trim_leading:
    StrCpy $R8 $R9 1
    StrCmp $R8 ";" 0 trim_trailing
      StrCpy $R9 $R9 "" 1
      Goto trim_leading

  trim_trailing:
    StrLen $R8 $R9
    IntCmp $R8 0 trim_done
    IntOp $R8 $R8 - 1
    StrCpy $R8 $R9 1 $R8
    StrCmp $R8 ";" 0 trim_done
      StrLen $R8 $R9
      IntOp $R8 $R8 - 1
      StrCpy $R9 $R9 $R8
      Goto trim_trailing

  trim_done:
    Pop $R8
    Exch $R9
FunctionEnd
