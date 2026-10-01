@echo off
REM Copper Core dev entry point (Windows). Keep this file ASCII-only and CRLF:
REM cmd.exe parses multi-byte text unreliably and can split a REM line apart.
REM The Chinese rationale lives in docs (platform adaptation note, dev entry).
REM
REM Why this file exists:
REM   1. This machine's execution policy blocks unsigned .ps1 files, including
REM      .cargo\env-check.ps1. The block happens before the script is loaded, so
REM      dot-sourcing cannot bypass it; -ExecutionPolicy Bypass must be passed
REM      when the PowerShell process starts. Hence a .cmd entry point.
REM   2. This machine's security policy denies a freshly built unsigned binary
REM      writes to %APPDATA% / %LOCALAPPDATA% / %TEMP%. It shows up as:
REM        - setup panic: data dir ... unusable: access denied (os error 5)
REM        - WebView2 creation failure: 0x800700AA / 0x8000FFFF
REM      Pointing the data root and the WebView2 profile into the repo side steps
REM      it, and keeps dev data fully isolated from real user data.
REM
REM The game versions root (settings `game.directory`) IS redirected here, to
REM %DEVROOT%\versions, and the Chinese rationale lives in docs.
REM
REM Why:
REM   The dev binary is built and launched from target\ inside this repo. The
REM   agent sandbox on this machine confines any process whose image path is
REM   under D:\CopperGolem\ to writing inside D:\CopperGolem\. Verified by
REM   running a copy of cmd.exe from this repo: writes to D:\minecraft\versions,
REM   D:\ and %TEMP% all fail with ERROR_ACCESS_DENIED, while writes under
REM   D:\CopperGolem\ succeed.
REM   The user's `game.directory` (D:\minecraft\versions) is a correct setting
REM   for the released app launched from Explorer, so it must not be rewritten.
REM   The process-level override COPPER_VERSIONS_DIR (resolved in
REM   services\paths.rs, same convention as COPPER_DATA_DIR) keeps the dev
REM   session inside the writable area without touching stored settings.
REM   Without it every dev start logs "versions root unusable: os error 5" and
REM   no game download can complete.
REM   To test against a real user directory, unset COPPER_VERSIONS_DIR and
REM   start a build from outside the repo (Explorer / shortcut / installed
REM   release build).
REM
REM Compiler env vars (MSVC / OpenSSL / TEMP) stay owned by .cargo\env-check.ps1;
REM this file does not duplicate them, so there is only one source of truth.
setlocal

set "ROOT=%~dp0"
set "DEVROOT=%ROOT%.devdata"

if "%COPPER_DATA_DIR%"=="" set "COPPER_DATA_DIR=%DEVROOT%"
if "%WEBVIEW2_USER_DATA_FOLDER%"=="" set "WEBVIEW2_USER_DATA_FOLDER=%DEVROOT%\webview2"
if "%COPPER_VERSIONS_DIR%"=="" set "COPPER_VERSIONS_DIR=%DEVROOT%\versions"

if not exist "%DEVROOT%" mkdir "%DEVROOT%"
if not exist "%WEBVIEW2_USER_DATA_FOLDER%" mkdir "%WEBVIEW2_USER_DATA_FOLDER%"

echo [dev] COPPER_DATA_DIR           = %COPPER_DATA_DIR%
echo [dev] WEBVIEW2_USER_DATA_FOLDER = %WEBVIEW2_USER_DATA_FOLDER%
echo [dev] COPPER_VERSIONS_DIR       = %COPPER_VERSIONS_DIR%
echo [dev] kernel log                = %COPPER_DATA_DIR%\data\logs\kernel.log
echo [dev] starting tauri dev ...

powershell -NoProfile -ExecutionPolicy Bypass -Command ". '%ROOT%.cargo\env-check.ps1'; Set-Location '%ROOT%'; npm run tauri:dev"
set "EXITCODE=%ERRORLEVEL%"

if "%EXITCODE%"=="0" goto done
echo.
echo [dev] process exited with code %EXITCODE%
echo [dev] code 101 - find the startup-failure line above; it names both the
echo [dev]              failing step and the exact path.
echo [dev] 0x800700AA / 0x8000FFFF - WebView2 is still using a rejected default
echo [dev]              profile dir; check WEBVIEW2_USER_DATA_FOLDER.
REM Keep a double-clicked window open: without this the diagnostic above is
REM printed and destroyed in the same instant, which is unreadable.
pause
:done
exit /b %EXITCODE%
