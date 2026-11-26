; ============================================================
;  WIN MAC installer (NSIS, per-user, no admin required)
;  Branding: maxodus
;  Build:  makensis -DVERSION=<ver> tools\winmac-setup.nsi
; ============================================================

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "nsDialogs.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "1.1.0"
!endif

; Repo root. The build workflow passes -DROOT=<workspace> (absolute); without it, fall
; back to the folder above the tools/ dir holding this script.
!ifndef ROOT
  !define ROOT "${__FILEDIR__}\.."
!endif

!define APPNAME   "WIN MAC"
!define COMPANY    "maxodus"
!define EXE       "winmac.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\WinMac"

Name "${APPNAME} ${VERSION}"
OutFile "${ROOT}\target\release\winmac-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\WIN MAC"
Icon "${ROOT}\resources\winmac.ico"
UninstallIcon "${ROOT}\resources\winmac.ico"
RequestExecutionLevel user
XPStyle on
BrandingText "maxodus"
SetCompressor lzma

; ---- Modern UI settings ----
!define MUI_ABORTWARNING
!define MUI_ICON "${ROOT}\resources\winmac.ico"
!define MUI_UNICON "${ROOT}\resources\winmac.ico"
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
Var OptLogin
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
  ${NSD_CreateCheckbox} 0 54u 100% 12u "Start WIN MAC automatically when you log in"
  Pop $OptLogin
  ${NSD_SetState} $OptLogin 0
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
  ; If a previous copy is running, its exe is locked and the File overwrite below would
  ; fail. Close it first (best-effort; harmless when nothing is running).
  DetailPrint "Closing any running WIN MAC..."
  nsExec::ExecToLog 'taskkill /F /T /IM winmac.exe'
  Sleep 500

  SetOutPath "$INSTDIR"
  File /oname=winmac.exe "${ROOT}\target\release\winmac.exe"
  File /oname=winmac.exe.sha256 "${ROOT}\target\release\winmac.exe.sha256"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; optional shortcuts — use explicit paths (the NSIS $DESKTOP/$SMPROGRAMS built-ins
  ; sometimes resolve to a OneDrive-redirected folder that silently fails).
  ; In silent mode (/S) the options page never runs, so no checkbox state exists: create the
  ; Start Menu shortcut (its default) and leave the rest (desktop, launch, login) off.
  ${If} ${Silent}
    CreateDirectory "$SMPROGRAMS\WIN MAC"
    CreateShortCut "$SMPROGRAMS\WIN MAC\WIN MAC.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0
  ${Else}
    ${NSD_GetState} $OptShortcut $0
    ${If} $0 == 1
      CreateDirectory "$SMPROGRAMS\WIN MAC"
      CreateShortCut "$SMPROGRAMS\WIN MAC\WIN MAC.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0
    ${EndIf}
    ${NSD_GetState} $OptDesktop $0
    ${If} $0 == 1
      ; resolve the real desktop folder (handles OneDrive redirection)
      StrCpy $0 "$DESKTOP"
      ReadRegStr $1 HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders" "Desktop"
      ${If} $1 != ""
        StrCpy $0 $1
      ${EndIf}
      CreateDirectory "$0"
      CreateShortCut "$0\WIN MAC.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0
    ${EndIf}
  ${EndIf}

  ; Add/Remove Programs entry (per-user)
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APPNAME} ${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${COMPANY}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${EXE},0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "https://github.com/Maxodus126/WinMac"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ; real installed size in KB (walk $INSTDIR)
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $0

  ; run at login / launch after install. The options page is skipped in silent mode
  ; (/S), so its checkboxes never exist there — read them only in interactive mode.
  ${IfNot} ${Silent}
    ${NSD_GetState} $OptLogin $0
    ${If} $0 == 1
      WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "WinMac" '"$INSTDIR\${EXE}"'
    ${EndIf}

    ${NSD_GetState} $OptLaunch $0
    ${If} $0 == 1
      ExecShell "" "$INSTDIR\${EXE}"
    ${EndIf}
  ${EndIf}
SectionEnd

; ============================================================
;  Uninstall section
; ============================================================
Section "Uninstall"
  ; Close WIN MAC FIRST: a running instance would (a) lock its exe so the Delete below
  ; fails, and (b) keep re-hiding the taskbar. Only after it is gone do we restore the
  ; taskbar, so nothing re-hides it.
  DetailPrint "Closing WIN MAC..."
  nsExec::ExecToLog 'taskkill /F /T /IM winmac.exe'
  Sleep 500

  IfFileExists "$INSTDIR\${EXE}" 0 skip_restore
    DetailPrint "Restoring the Windows taskbar..."
    nsExec::ExecToLog '"$INSTDIR\${EXE}" --restore'
    Sleep 300
  skip_restore:

  ; resolve the real desktop folder (handles OneDrive redirection), matching the installer
  StrCpy $0 "$DESKTOP"
  ReadRegStr $1 HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders" "Desktop"
  ${If} $1 != ""
    StrCpy $0 $1
  ${EndIf}
  Delete "$0\WIN MAC.lnk"
  RMDir /r "$SMPROGRAMS\WIN MAC"

  ; remove the run-at-login entry (harmless if it was never set)
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "WinMac"

  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\${EXE}.sha256"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINSTALL_KEY}"

  ; data removal only with explicit consent; silent uninstall keeps user data (the safe
  ; default — nothing is destroyed without the user asking for it)
  ${IfNot} ${Silent}
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 \
      "Do you also want to delete your WIN MAC settings and data?$\r$\nChoose No to keep them." \
      IDYES destroy_data
    Goto keep_data
    destroy_data:
      DetailPrint "Removing WIN MAC application data..."
      RMDir /r "$APPDATA\WinMac"
      Goto data_done
    keep_data:
      DetailPrint "Keeping WIN MAC application data."
    data_done:
  ${Else}
    DetailPrint "Keeping WIN MAC application data."
  ${EndIf}
SectionEnd
