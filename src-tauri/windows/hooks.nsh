; NSIS transition hooks for migrating from Electron (electron-builder) to Tauri.
;
; 1. Pre-install: If an existing Electron per-user installation is present at
;    "$LOCALAPPDATA\Programs\biu\Uninstall Biu.exe", silently uninstall it
;    while preserving "%APPDATA%\Biu" (--updated /KEEP_APP_DATA) so the user
;    does not end up with duplicate Biu shortcuts or Add/Remove Programs entries.
; 2. Post-install: When launched by electron-updater's quitAndInstall
;    (which passes `/S --updated --force-run` instead of Tauri's `/R`),
;    launch the newly installed Tauri executable after installation succeeds.

!macro NSIS_HOOK_PREINSTALL
  IfFileExists "$LOCALAPPDATA\Programs\biu\Uninstall Biu.exe" 0 +6
    DetailPrint "Removing legacy Electron Biu installation..."
    CopyFiles /SILENT "$LOCALAPPDATA\Programs\biu\Uninstall Biu.exe" "$PLUGINSDIR\electron-uninstaller.exe"
    ExecWait '"$PLUGINSDIR\electron-uninstaller.exe" /S /KEEP_APP_DATA /currentuser --updated _?=$LOCALAPPDATA\Programs\biu' $R9
    Delete "$LOCALAPPDATA\Programs\biu\Uninstall Biu.exe"
    RMDir /r "$LOCALAPPDATA\Programs\biu"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} ${Silent}
    ClearErrors
    ${GetOptions} $CMDLINE "/R" $R0
    ${If} ${Errors}
      ClearErrors
      ${GetOptions} $CMDLINE "--force-run" $R0
      ${IfNot} ${Errors}
        nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend
