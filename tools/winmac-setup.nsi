; ============================================================
;  WIN MAC installer (NSIS, per-user, no admin required)
;  Branding: maxodus
;  Build:  makensis -DVERSION=<ver> tools\winmac-setup.nsi
; ============================================================

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "nsDialogs.nsh"

!ifndef VERSION
  !define VERSION "1.0.2"
!endif

!define APPNAME   "WIN MAC"
!define COMPANY    "maxodus"
!define EXE       "winmac.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinMac"

Name "${APPNAME} ${VERSION}"
OutFile "target\release\winmac-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\WIN MAC"
Icon "resources\winmac.ico"
UninstallIcon "resources\winmac.ico"
RequestExecutionLevel user
XPStyle on
BrandingText "maxodus"
SetCompressor lzma

; ---- Modern UI settings ----
!define MUI_ABORTWARNING
!define MUI_ICON "resources\winmac.ico"
!define MUI_UNICON "resources\winmac.ico"
!define MUI_WELCOMEPAGE_TITLE "Welcome to WIN MAC ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "A macOS-style dock and menu bar for Windows 11.$\r$\n$\r$\nDeveloped by maxodus.$\r$\nThis installer will place WIN MAC in your own user folder — no administrator rights required."
!define MUI_FINISHPAGE_TITLE "Installation complete"
!define MUI_FINISHPAGE_TEXT "WIN MAC ${VERSION} has been installed for your user.$\r$\nUse the Start Menu shortcut or run winmac.exe to start it."
!define MUI_FINISHPAGE_NOREBOOTSUPPORT

; ---- Pages: welcome, consent, options, install, finish ----
!insertmacro MUI_PAGE_WELCOME

; --- consent page: a checkbox that MUST be checked to continue ---
Var ConsentCheck
Function ConsentCreate
  !insertmacro MUI_HEADER_TEXT "License Agreement" "Please read and accept"
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 60u \
    "WIN MAC is licensed under the MIT License.$\r$\n$\r$\n" \
    "Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files, to deal in the Software without restriction.$\r$\n$\r$\n" \
    "The software is provided 'AS IS', without warranty of any kind."
  Pop $0
  ${NSD_CreateCheckbox} 0 65u 100% 12u "I have read and accept the license agreement"
  Pop $ConsentCheck
  nsDialogs::Show
FunctionEnd

Function ConsentLeave
  ${NSD_GetState} $ConsentCheck $0
  ${If} $0 == 0
    MessageBox MB_ICONEXCLAMATION|MB_OK "You must accept the license agreement to continue."
    Abort
  ${EndIf}
FunctionEnd

Page custom ConsentCreate ConsentLeave

; --- options page: checkboxes to choose shortcuts / launch ---
Var OptShortcut
Var OptDesktop
Var OptLaunch
Function OptCreate
  !insertmacro MUI_HEADER_TEXT "Installation options" "Choose what you want"
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${NSD_CreateCheckbox} 0 0 100% 12u "Create a Start Menu shortcut"
  Pop $OptShortcut
  ${NSD_SetState} $OptShortcut 1
  ${NSD_CreateCheckbox} 0 18u 100% 12u "Create a desktop shortcut"
  Pop $OptDesktop
  ${NSD_SetState} $OptDesktop 0
  ${NSD_CreateCheckbox} 0 36u 100% 12u "Launch WIN MAC after installation"
  Pop $OptLaunch
  ${NSD_SetState} $OptLaunch 1
  nsDialogs::Show
FunctionEnd

Function OptLeave
FunctionEnd

Page custom OptCreate OptLeave

!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

; ---- Uninstall pages ----
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

!insertmacro MUI_LANGUAGE "English"

; ============================================================
;  Install section
; ============================================================
Section "Install"
  SetOutPath "$INSTDIR"
  File /oname=winmac.exe "target\release\winmac.exe"
  File /oname=winmac.exe.sha256 "target\release\winmac.exe.sha256"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; optional shortcuts
  ${NSD_GetState} $OptShortcut $0
  ${If} $0 == 1
    CreateDirectory "$SMPROGRAMS\WIN MAC"
    CreateShortCut "$SMPROGRAMS\WIN MAC\WIN MAC.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0
  ${EndIf}
  ${NSD_GetState} $OptDesktop $0
  ${If} $0 == 1
    CreateShortCut "$DESKTOP\WIN MAC.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0
  ${EndIf}

  ; Add/Remove Programs entry (per-user)
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APPNAME} ${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${COMPANY}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${EXE},0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" 2048

  ; launch after install, if chosen
  ${NSD_GetState} $OptLaunch $0
  ${If} $0 == 1
    ExecShell "" "$INSTDIR\${EXE}"
  ${EndIf}
SectionEnd

; ============================================================
;  Uninstall section
; ============================================================
Section "Uninstall"
  ; restore the Windows taskbar first (best-effort), then remove files
  IfFileExists "$INSTDIR\${EXE}" 0 +2
    nsExec::ExecToLog '"$INSTDIR\${EXE}" --restore'

  Delete "$DESKTOP\WIN MAC.lnk"
  RMDir /r "$SMPROGRAMS\WIN MAC"

  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\${EXE}.sha256"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINSTALL_KEY}"
SectionEnd
