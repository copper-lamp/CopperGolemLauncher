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
REM   2. This machine's security policy denies writes to %APPDATA% /
REM      %LOCALAPPDATA% / %TEMP% for freshly built unsigned binaries, which shows
REM      up as two seemingly unrelated failures:
REM        - setup panic: data dir ... unusable: access denied (os error 5)
REM        - WebView2 creation failure: 0x800700AA / 0x8000FFFF
REM      Pointing both the data root and the WebView2 profile into the repo side
REM      steps it, and keeps dev data fully isolated from real user data.
REM
REM Compiler env vars (MSVC / OpenSSL / TEMP) stay owned by .cargo\env-check.ps1;
REM this file does not duplicate them, so there is only one source of truth.
setlocal

set "ROOT=%~dp0"
set "DEVROOT=%ROOT%.devdata"

if "%COPPER_DATA_DIR%"=="" set "COPPER_DATA_DIR=%DEVROOT%"
if "%WEBVIEW2_USER_DATA_FOLDER%"=="" set "WEBVIEW2_USER_DATA_FOLDER=%DEVROOT%\webview2"

if not exist "%DEVROOT%" mkdir "%DEVROOT%"
if not exist "%WEBVIEW2_USER_DATA_FOLDER%" mkdir "%WEBVIEW2_USER_DATA_FOLDER%"

echo [dev] COPPER_DATA_DIR           = %COPPER_DATA_DIR%
echo [dev] WEBVIEW2_USER_DATA_FOLDER = %WEBVIEW2_USER_DATA_FOLDER%
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
:done
exit /b %EXITCODE%
