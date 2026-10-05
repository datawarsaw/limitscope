; LimitScope installer hooks (Tauri NSIS `installerHooks`).
;
; The product was renamed from "Rate Limits" to "LimitScope" in v0.5. The
; rename changes the NSIS product identity (ARP key, default install dir,
; main binary name), so without these hooks the branded installer would
; create a second installed product instead of upgrading the existing one.
;
; These hooks upgrade a prior "Rate Limits" install IN PLACE:
;   1. detect the old product's uninstall entry (HKCU first, then HKLM —
;      v0.4/v0.5 installs used the per-user Tauri default);
;   2. point $INSTDIR at the old install location before any file is copied;
;   3. stop the old product's process, silently uninstall it (removes its
;      ARP entry and old files), then let the branded install proceed.
;
; User data is never touched: settings (WebView2 profile storage), Rust
; quota history, and notification state all live under
; %LOCALAPPDATA%\com.ratelimits.desktop, keyed by the app identifier that
; this migration deliberately preserves.
;
; Autostart is migrated in NSIS_HOOK_POSTINSTALL: the Run-key value is
; named after the app_name the autostart plugin registers ("Rate Limits"
; before the rename, "LimitScope" after), and the app reconciles the OS
; state against the stored preference at launch — an unmigrated entry
; would make enabled autostart silently read as disabled. The hook moves
; an existing entry to the new value name pointing at the new binary.

!define LIMITSCOPE_OLD_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rate Limits"
!define LIMITSCOPE_OLD_RUN_NAME "Rate Limits"
!define LIMITSCOPE_RUN_NAME "LimitScope"
!define LIMITSCOPE_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

; The Tauri NSIS template stores InstallLocation and UninstallString with
; surrounding quotes ("C:\...\Rate Limits"). Copying those values into
; $INSTDIR verbatim corrupts every downstream path (NSIS keeps the quote and
; drops the drive colon), so both reads are stripped before use. Found by
; the v0.5.0 RC upgrade smoke against a real v0.4 install.
!macro LIMITSCOPE_UNQUOTE _in _out
  StrCpy ${_out} ${_in}
  StrCpy $R8 "${_in}" 1
  ${If} $R8 == "$\""
    StrCpy ${_out} "${_in}" "" 1
  ${EndIf}
  StrLen $R8 "${_out}"
  ${If} $R8 > 1
    IntOp $R8 $R8 - 1
    StrCpy $R9 "${_out}" 1 $R8
    ${If} $R9 == "$\""
      StrCpy ${_out} "${_out}" $R8
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; Capture the old autostart entry BEFORE the old uninstaller runs: the
  ; old product's autostart plugin removes its own Run value during silent
  ; uninstall, so POSTINSTALL can no longer see whether autostart was
  ; enabled. $R7 survives across both hooks in the same installer process.
  ReadRegStr $R7 HKCU "${LIMITSCOPE_RUN_KEY}" "${LIMITSCOPE_OLD_RUN_NAME}"
  ReadRegStr $R0 SHCTX "${LIMITSCOPE_OLD_KEY}" "InstallLocation"
  ${If} $R0 == ""
    ReadRegStr $R0 HKLM "${LIMITSCOPE_OLD_KEY}" "InstallLocation"
  ${EndIf}
  !insertmacro LIMITSCOPE_UNQUOTE $R0 $R0
  ${If} $R0 != ""
    ; Upgrade in place: reuse the old install directory before extraction.
    ; The template runs SetOutPath $INSTDIR just before this hook, so the
    ; output path has to follow the redirected directory.
    StrCpy $INSTDIR $R0
    SetOutPath $INSTDIR
    ; A running old build would lock its files against the silent uninstall.
    ; (Pre-rename installs shipped the cargo binary name rate-limits.exe;
    ; the branded build installs as LimitScope.exe via mainBinaryName.)
    nsExec::Exec 'taskkill /F /IM "rate-limits.exe"'
    ReadRegStr $R1 SHCTX "${LIMITSCOPE_OLD_KEY}" "UninstallString"
    ${If} $R1 == ""
      ReadRegStr $R1 HKLM "${LIMITSCOPE_OLD_KEY}" "UninstallString"
    ${EndIf}
    !insertmacro LIMITSCOPE_UNQUOTE $R1 $R1
    ${If} $R1 != ""
      ; _?= runs the old uninstaller synchronously from its own dir; it
      ; removes the old ARP entry and files but not the app-data directory.
      ; The path is quoted explicitly around the unquoted registry value.
      ExecWait '"$R1" /S _?=$INSTDIR'
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Autostart migration: only an existing old entry proves the user had
  ; Autostart migration from the value captured in PREINSTALL (see above):
  ; a captured entry proves the user had autostart enabled; a fresh install
  ; captured nothing and has nothing to move.
  ${If} $R7 != ""
    DeleteRegValue HKCU "${LIMITSCOPE_RUN_KEY}" "${LIMITSCOPE_OLD_RUN_NAME}"
    WriteRegStr HKCU "${LIMITSCOPE_RUN_KEY}" "${LIMITSCOPE_RUN_NAME}" '"$INSTDIR\LimitScope.exe" --hidden'
  ${EndIf}
!macroend
