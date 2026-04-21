; Hebrew.nsh — Custom NSIS language strings for the Tauri bundler.
;
; NSIS keeps two separate string tables: one for the installer (`LangString`)
; and one for the uninstaller (`UninstallLangString`). If a string is defined
; only via `LangString`, every `$(...)` reference inside an `un.*` function
; (e.g. the "Delete app data" checkbox on the uninstall Confirm page) resolves
; to an empty string. That produces the broken "checkbox with no label" bug.
;
; This file defines each custom Tauri string in BOTH tables via the
; `TauriLangString` macro below, so the same Hebrew text covers every page
; in the wizard and in the uninstaller.
;
; Keep every string in this list synchronised with the Tauri bundler's
; English.nsh (https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/windows/nsis/languages/English.nsh).
; The CI check in `scripts/check-nsis-translations.py` enforces this.

!macro TauriLangString NAME TEXT
  LangString ${NAME} ${LANG_HEBREW} "${TEXT}"
  UninstallLangString ${NAME} ${LANG_HEBREW} "${TEXT}"
!macroend

!insertmacro TauriLangString addOrReinstall "הוספה/התקנה מחדש של רכיבים"
!insertmacro TauriLangString alreadyInstalled "כבר מותקן"
!insertmacro TauriLangString alreadyInstalledLong "${PRODUCTNAME} ${VERSION} כבר מותקן. בחר את הפעולה הרצויה ולחץ על 'הבא' להמשך."
!insertmacro TauriLangString appRunning "{{product_name}} פעיל! סגור אותו תחילה ונסה שוב."
!insertmacro TauriLangString appRunningOkKill "{{product_name}} פעיל!$\nלחץ אישור כדי לסגור אותו"
!insertmacro TauriLangString chooseMaintenanceOption "בחר את פעולת התחזוקה לביצוע."
!insertmacro TauriLangString choowHowToInstall "בחר כיצד להתקין את ${PRODUCTNAME}."
!insertmacro TauriLangString createDesktop "צור קיצור דרך בשולחן העבודה"
!insertmacro TauriLangString deleteAppData "מחק את נתוני האפליקציה"
!insertmacro TauriLangString dontUninstall "אל תסיר"
!insertmacro TauriLangString dontUninstallDowngrade "אל תסיר (שדרוג-לאחור בלי הסרה מושבת במתקין זה)"
!insertmacro TauriLangString failedToKillApp "לא ניתן לסגור את {{product_name}}. סגור אותו תחילה ונסה שוב"
!insertmacro TauriLangString installingWebview2 "מתקין WebView2..."
!insertmacro TauriLangString newerVersionInstalled "גרסה חדשה יותר של ${PRODUCTNAME} כבר מותקנת. לא מומלץ להתקין גרסה ישנה יותר. אם בכל זאת ברצונך להמשיך, רצוי להסיר קודם את הגרסה הנוכחית. בחר פעולה ולחץ על 'הבא'."
!insertmacro TauriLangString older "ישנה יותר"
!insertmacro TauriLangString olderOrUnknownVersionInstalled "גרסה $R4 של ${PRODUCTNAME} מותקנת במערכת. רצוי להסיר את הגרסה הנוכחית לפני ההתקנה. בחר פעולה ולחץ על 'הבא'."
!insertmacro TauriLangString silentDowngrades "שדרוג-לאחור מושבת במתקין זה. לא ניתן להמשיך במצב שקט — יש להשתמש במתקין הגרפי.$\n"
!insertmacro TauriLangString unableToUninstall "לא ניתן להסיר!"
!insertmacro TauriLangString uninstallApp "הסר את ${PRODUCTNAME}"
!insertmacro TauriLangString uninstallBeforeInstalling "הסר לפני התקנה"
!insertmacro TauriLangString unknown "לא ידועה"
!insertmacro TauriLangString webview2AbortError "התקנת WebView2 נכשלה. האפליקציה לא תפעל בלעדיו. נסה להריץ את המתקין שוב."
!insertmacro TauriLangString webview2DownloadError "שגיאה: הורדת WebView2 נכשלה - $0"
!insertmacro TauriLangString webview2DownloadSuccess "WebView2 הורד בהצלחה"
!insertmacro TauriLangString webview2Downloading "מוריד את WebView2..."
!insertmacro TauriLangString webview2InstallError "שגיאה: התקנת WebView2 נכשלה עם קוד $1"
!insertmacro TauriLangString webview2InstallSuccess "WebView2 הותקן בהצלחה"
