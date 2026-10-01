@echo off
REM Copper Golem Android APK build entry point.
REM
REM Keep this file ASCII-only and CRLF: the previous revision carried mangled
REM Chinese comments whose stray bytes were executed as commands, which broke the
REM `set` lines below and made the Tauri CLI fall back to a non-existent SDK path.
REM
REM Why each variable is needed:
REM   ANDROID_HOME / ANDROID_SDK_ROOT : Tauri CLI locates the SDK and build-tools
REM   NDK_HOME / ANDROID_NDK_ROOT     : cargo-ndk / cc-rs locate clang wrappers
REM   JAVA_HOME                       : Gradle needs a JDK 17+
REM   PATH                            : msys2 make + perl are required to build the
REM                                     vendored OpenSSL for the Android target
REM   CC_/CXX_/AR_/RANLIB_ _aarch64_linux_android
REM                                   : MUST use forward slashes. msys2's shell
REM                                     eats backslashes, so the previous
REM                                     revision produced
REM                                     "D:androidndk...clang.exe: No such file".
REM   CARGO_HOME                      : kept inside the repository on purpose.
REM                                     `tauri-plugin`'s build script runs
REM                                     create_dir_all(<registry crate>/android/.tauri),
REM                                     i.e. it writes into the unpacked cargo
REM                                     registry under %USERPROFILE%\.cargo. When
REM                                     that tree is not writable (locked-down or
REM                                     sandboxed machine) every tauri plugin
REM                                     fails with "failed to create .tauri
REM                                     directory: os error 5". Pointing CARGO_HOME
REM                                     at .cargohome next to this repo keeps all
REM                                     writes inside the working tree.
REM                                     Seed it once with:
REM                                       robocopy %USERPROFILE%\.cargo .cargohome /E
setlocal

set "ANDROID_HOME=D:/android"
set "ANDROID_SDK_ROOT=D:/android"
set "NDK_HOME=D:/android/ndk/27.3.13750724"
set "ANDROID_NDK_ROOT=D:/android/ndk/27.3.13750724"
set "JAVA_HOME=D:/jdk"

set "TC=D:/android/ndk/27.3.13750724/toolchains/llvm/prebuilt/windows-x86_64/bin"
set "CC_aarch64_linux_android=%TC%/aarch64-linux-android24-clang.cmd"
set "CXX_aarch64_linux_android=%TC%/aarch64-linux-android24-clang++.cmd"
set "AR_aarch64_linux_android=%TC%/llvm-ar.exe"
set "RANLIB_aarch64_linux_android=%TC%/llvm-ranlib.exe"

set "PATH=C:\msys64\usr\bin;C:\Program Files\Git\usr\bin;D:\jdk\bin;D:\android\platform-tools;%PATH%"
set "TEMP=D:\CopperGolem\CopperCore\target\tmp"
set "TMP=%TEMP%"
if not exist "%TEMP%" mkdir "%TEMP%"

set "CARGO_HOME=D:\CopperGolem\CopperCore\.cargohome"
if not exist "%CARGO_HOME%\registry" (
    echo [apk] CARGO_HOME is not seeded: %CARGO_HOME%
    echo [apk] run: robocopy "%USERPROFILE%\.cargo" "%CARGO_HOME%" /E
    exit /b 2
)

cd /d D:\CopperGolem\CopperCore
echo [apk] SDK        = %ANDROID_HOME%
echo [apk] NDK        = %NDK_HOME%
echo [apk] JDK        = %JAVA_HOME%
echo [apk] toolchain  = %TC%
echo [apk] CARGO_HOME = %CARGO_HOME%
echo [apk] building debug APK for aarch64 ...

call node "node_modules\@tauri-apps\cli\tauri.js" android build --apk --debug --target aarch64 --features vendored-openssl
set "EXITCODE=%ERRORLEVEL%"
echo [apk] exit=%EXITCODE%
exit /b %EXITCODE%
