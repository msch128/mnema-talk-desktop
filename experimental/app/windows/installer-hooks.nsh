; Hooks of the Windows installer (Tauri's NSIS template, tauri.conf.json
; bundle.windows.nsis.installerHooks).
;
; Up to 1.3.5 the app was called "Mnema Desktop DEV". The installer names its
; folder, its entry under Installed apps and its shortcuts after the product, so
; without this an update from such a version would install a second copy beside
; the old one and leave the old entry and shortcuts behind.
;
; Before installing, a copy installed under the old name is removed by its own
; uninstaller, run silently. That keeps the app's data and sessions: they are
; under the identifier (com.mnema.desktop.shareddev), which did not change, and
; the uninstaller deletes them only when asked in its dialog. The new version
; then goes into the new default folder, or into the old folder if that was
; chosen by hand, and the Start menu and desktop shortcuts the old copy had are
; made again under the new name (an update makes no shortcuts by itself).
;
; Checked on a Windows runner by scripts/test-windows-rename-update.ps1.

!define MNEMA_OLD_PRODUCT "Mnema Desktop DEV"
!define MNEMA_OLD_UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${MNEMA_OLD_PRODUCT}"

Var MnemaOldDir
Var MnemaOldStartMenu
Var MnemaOldDesktop

!macro NSIS_HOOK_PREINSTALL
  Push $0
  StrCpy $MnemaOldStartMenu 0
  StrCpy $MnemaOldDesktop 0
  ; The installer installs per user, so the old entry is under HKCU.
  ReadRegStr $MnemaOldDir HKCU "${MNEMA_OLD_UNINSTKEY}" "InstallLocation"
  ; Written with quotes around it.
  StrCpy $0 $MnemaOldDir 1
  ${If} $0 == '"'
    StrCpy $MnemaOldDir $MnemaOldDir "" 1
    StrCpy $MnemaOldDir $MnemaOldDir -1
  ${EndIf}
  ${If} $MnemaOldDir != ""
  ${AndIf} ${FileExists} "$MnemaOldDir\uninstall.exe"
    ${If} ${FileExists} "$SMPROGRAMS\${MNEMA_OLD_PRODUCT}.lnk"
      StrCpy $MnemaOldStartMenu 1
    ${EndIf}
    ${If} ${FileExists} "$DESKTOP\${MNEMA_OLD_PRODUCT}.lnk"
      StrCpy $MnemaOldDesktop 1
    ${EndIf}
    ; A folder chosen by hand stays; the default one takes the new name.
    ${If} $INSTDIR == "$LOCALAPPDATA\${PRODUCTNAME}"
    ${AndIf} $MnemaOldDir != "$LOCALAPPDATA\${MNEMA_OLD_PRODUCT}"
      StrCpy $INSTDIR $MnemaOldDir
    ${EndIf}
    ; Run in place (`_?=`), so that it is done when ExecWait returns; it then
    ; cannot delete itself and its folder, which happens below.
    ExecWait '"$MnemaOldDir\uninstall.exe" /S _?=$MnemaOldDir' $0
    Delete "$MnemaOldDir\uninstall.exe"
    ${If} $MnemaOldDir != $INSTDIR
      RMDir "$MnemaOldDir"
    ${EndIf}
    ; What the old uninstaller leaves when it is not asked to delete the data:
    ; the record of its folder and installer language.
    DeleteRegKey HKCU "${MNEMA_OLD_UNINSTKEY}"
    DeleteRegKey HKCU "Software\${MANUFACTURER}\${MNEMA_OLD_PRODUCT}"
    SetOutPath $INSTDIR
  ${EndIf}
  Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $MnemaOldStartMenu = 1
  ${AndIfNot} ${FileExists} "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  ${EndIf}
  ${If} $MnemaOldDesktop = 1
  ${AndIfNot} ${FileExists} "$DESKTOP\${PRODUCTNAME}.lnk"
    CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
  ${EndIf}
!macroend
