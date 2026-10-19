; Keep the previous executable for rollback during the 1.44 -> 1.45 update.
; AppData is neither removed nor relocated by this hook.
!macro NSIS_HOOK_PREINSTALL
  ReadRegStr $R8 SHCTX "${UNINSTKEY}" "DisplayVersion"
  ${If} $R8 == "1.44.0"
    IfFileExists "$INSTDIR\local-planner.exe" 0 sfery_backup_done
    IfFileExists "$LOCALAPPDATA\${BUNDLEID}\rollback\1.44.0\local-planner.exe" sfery_backup_done 0
    CreateDirectory "$LOCALAPPDATA\${BUNDLEID}\rollback\1.44.0"
    ClearErrors
    CopyFiles /SILENT "$INSTDIR\local-planner.exe" "$LOCALAPPDATA\${BUNDLEID}\rollback\1.44.0\local-planner.exe"
    ${If} ${Errors}
      MessageBox MB_OK|MB_ICONSTOP "Cannot back up Sfery 1.44.0. Update cancelled."
      Abort
    ${EndIf}
  ${EndIf}
  sfery_backup_done:
!macroend
