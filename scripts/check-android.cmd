@echo off
REM Fast Android type check for copper-core (no APK, no Gradle).
REM
REM Use this to catch Rust errors in ~1 minute instead of paying the full
REM `scripts\build-android-apk.cmd` cycle (Rust + Gradle).
REM
REM It deliberately mirrors the build script's environment, because a mismatched
REM CARGO_HOME or TEMP produces environment errors that look like code errors:
REM   CARGO_HOME : the local .cargohome, so registry writes stay inside the repo
REM                (see the comment block in build-android-apk.cmd)
REM   TEMP/TMP   : target\tmp, because the default temp dir can be unwritable
REM   CFLAGS     : cleared, so MSVC flags do not leak into the NDK clang
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
set "CFLAGS="

set "PATH=C:\msys64\usr\bin;C:\Program Files\Git\usr\bin;D:\jdk\bin;D:\android\platform-tools;%PATH%"
set "TEMP=D:\CopperGolem\CopperCore\target\tmp"
set "TMP=%TEMP%"
set "CARGO_HOME=D:\CopperGolem\CopperCore\.cargohome"

cd /d D:\CopperGolem\CopperCore
echo [android-check] target=aarch64-linux-android CARGO_HOME=%CARGO_HOME%
cargo check -p copper-core --lib --target aarch64-linux-android
set "EXITCODE=%ERRORLEVEL%"
echo [android-check] exit=%EXITCODE%
exit /b %EXITCODE%
