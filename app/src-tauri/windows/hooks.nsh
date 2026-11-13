; NSIS installer hooks (bundle > windows > nsis > installerHooks).
; Tauri registers the .ferry association under the class "Ferry share" with the app's own icon;
; give the files their ticket icon instead. The class key is removed again on uninstall.

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "Software\Classes\Ferry share\DefaultIcon" "" "$INSTDIR\ferry-file.ico,0"
  ; Tell Explorer that file associations changed, so the new icon shows right away.
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend
