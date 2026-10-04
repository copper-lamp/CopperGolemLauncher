# Android cross-compilation environment (this machine).
#
# IMPORTANT: this file MUST stay pure ASCII (no BOM, no non-ASCII bytes).
# PowerShell 5.1 reads BOM-less .ps1 files using the system ANSI codepage
# (GBK on this machine). Any Chinese comment in here gets mangled into invalid
# bytes and the whole script dies with a ParserError -- and because it is
# dot-sourced, the failure is SILENT-ish: cargo then runs with MSVC CFLAGS and
# no NDK compiler, failing deep inside cc-rs with confusing errors such as
# "clang: error: no such file or directory: '/MD'".
# The Chinese version of these notes lives in docs/build-android.md.
#
# Usage (from the CopperCore directory):
#   powershell -NoProfile -ExecutionPolicy Bypass -Command `
#     ". '.cargo\env-check.ps1'; . '.cargo\env-android.ps1'; cargo check -p copper-core --lib --target aarch64-linux-android"
#
# Why this is a separate file from env-check.ps1:
#   - env-check.ps1 owns the DESKTOP MSVC build (D8050, OpenSSL dir, TEMP).
#   - This file owns CROSS COMPILING to aarch64-linux-android.
#   They must not be merged: env-check.ps1 exports CFLAGS containing MSVC-only
#   flags ("/MD /O2 /Brepro"). Those flags are rejected by the NDK clang.
#
# Traps already hit on this machine (documented so they are not retried):
#   1. CFLAGS must be cleared here, otherwise MSVC flags pollute NDK clang.
#   2. cc-rs cannot locate the NDK compiler from PATH alone: msys' plain
#      "clang" wins and it does not accept --target=aarch64-linux-android.
#      We must point at the target-prefixed NDK driver wrappers explicitly.
#   3. Keep the toolchain paths FORWARD-SLASHED. msys (used by make/perl for the
#      vendored OpenSSL build) eats backslashes, which produces a CC like
#      "D:androidndk...binclang.exe" and make Error 127.
#   4. OPENSSL_* must be scrubbed, because env-check.ps1 points OPENSSL_DIR at the
#      WINDOWS OpenSSL and openssl-sys would happily try to link those PE libs
#      into the Android target.
#
# NOTE: this script assumes env-check.ps1 was dot-sourced FIRST (it owns the
# desktop MSVC settings and the workspace-local CARGO_HOME/TEMP), because this
# file deliberately clears some of the variables env-check.ps1 sets.
# NOTE: do NOT leave $ErrorActionPreference = 'Stop' set for the caller.
# This script is dot-sourced. With 'Stop' still in effect, PowerShell turns the
# native cargo/gradle progress lines written to stderr into terminating
# NativeCommandError records, and the build dies at the first "Checking ..."
# line with no actual compiler diagnostic. Scope it to the validation below.
$ErrorActionPreference = 'Stop'

$ndk = 'D:\android\ndk\27.3.13750724'
$tc = "$ndk\toolchains\llvm\prebuilt\windows-x86_64\bin"

if (-not (Test-Path -LiteralPath $tc)) {
    throw "[env-android] NDK toolchain not found: $tc (edit this script or install NDK r27 LTS)"
}

$env:ANDROID_HOME     = 'D:\android'
$env:ANDROID_SDK_ROOT = 'D:\android'
$env:NDK_HOME         = $ndk
$env:ANDROID_NDK_ROOT = $ndk
$env:JAVA_HOME        = 'D:\jdk'

# Trap 1: MSVC flags must not reach the NDK clang -- but CFLAGS must NOT be left
#   empty either. env-check.ps1 exports "/MD /O2 /Brepro /DSQLITE_CORE", which the
#   NDK clang rejects, so we clear it and put the one flag that actually matters
#   for a cross build in its place.
#   This is not cosmetic: OpenSSL's Configure folds the inherited CFLAGS into the
#   generated Makefile, and when CFLAGS is empty the Makefile ends up with
#   "CFLAGS=-Wall -O3" and NO --target. clang then assumes the host triple, so
#   every ARM assembly file dies with
#     crypto/arm_arch.h:43: error: "unsupported ARM architecture"
#   because __aarch64__ is never defined. Passing --target through CFLAGS is what
#   makes the vendored OpenSSL build produce aarch64 objects.
$env:CFLAGS   = '--target=aarch64-linux-android24'
$env:CXXFLAGS = '--target=aarch64-linux-android24'

# Trap 1b: env-check.ps1 points OPENSSL_DIR at the WINDOWS OpenSSL (F:\OpenSSL-Win64).
# openssl-sys happily uses it for the aarch64-linux-android target too and then
# dies with "OpenSSL libdir ... does not contain the required files", because
# those are PE libs, not Android ELF ones. There is no Android OpenSSL on this
# machine, so scrub every OPENSSL_* override and let the `vendored-openssl`
# feature build OpenSSL from source with the NDK toolchain instead.
foreach ($v in @(
    'OPENSSL_DIR',
    'OPENSSL_INCLUDE_DIR',
    'OPENSSL_LIB_DIR',
    'OPENSSL_STATIC',
    'AARCH64_LINUX_ANDROID_OPENSSL_DIR',
    'AARCH64_LINUX_ANDROID_OPENSSL_INCLUDE_DIR',
    'AARCH64_LINUX_ANDROID_OPENSSL_LIBS',
    'AARCH64_LINUX_ANDROID_OPENSSL_STATIC'
)) {
    Remove-Item -Path "Env:$v" -ErrorAction SilentlyContinue
}

# Trap 2: the compiler must be the NDK one, not msys' plain "clang" (which does
#   not understand --target=aarch64-linux-android).
#
# Trap 3: use BARE names ("clang"), not full paths to the target-prefixed
#   wrappers. The vendored OpenSSL build (openssl-src) passes CC to OpenSSL's
#   Configure, which on Windows rewrites a pathed CC into
#       <dir>\clang.exe
#   -- note the injected BACKSLASH. That single backslash is fatal: OpenSSL's
#   make recipes run under msys /bin/sh, where "\c", "\n", "\t" are escapes, so
#   the compiler resolves to "D:androidndk...binclang.exe" and make dies with
#   Error 127. Observed both with backslashed paths ("D:androidndk...") and with
#   forward-slashed ones (".../bin\clang.exe").
#
#   A bare name has no directory to normalize, so Configure leaves it alone and
#   /bin/sh resolves it through PATH -- provided the NDK bin dir comes BEFORE
#   msys in PATH, otherwise msys' own clang wins and gets no --target.
#   This is safe because OpenSSL's generated Makefile already carries
#   CFLAGS=--target=aarch64-linux-android24 (verified in the generated Makefile),
#   and cc-rs passes --target=<triple> itself when cross compiling. So the plain
#   NDK clang is exactly the right driver; the aarch64-linux-android24-clang.cmd
#   wrappers only add an API-level floor that minSdk=26 already exceeds.
$ndkBin = 'D:/android/ndk/27.3.13750724/toolchains/llvm/prebuilt/windows-x86_64/bin'

$env:CC_aarch64_linux_android     = 'clang'
$env:CXX_aarch64_linux_android    = 'clang++'
$env:AR_aarch64_linux_android     = "$ndkBin/llvm-ar.exe"
$env:RANLIB_aarch64_linux_android = "$ndkBin/llvm-ranlib.exe"

# Trap 4: cargo defaults to "cc" as the linker for unknown targets, and "cc"
#   does not exist on Windows, so the final link fails with
#     error: linker `cc` not found / program not found
#   Point it at the NDK's target-prefixed clang driver. Unlike CC above this may
#   safely be an absolute native path: cargo executes the linker directly, it does
#   not go through msys sh, so backslashes are not mangled here. The wrapper also
#   supplies --sysroot and the API-level floor for us.
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "$ndk\toolchains\llvm\prebuilt\windows-x86_64\bin\aarch64-linux-android24-clang.cmd"

# msys provides make / perl, required by the vendored OpenSSL build.
# The NDK bin dir must precede msys, see trap 3.
$env:PATH = "$ndkBin;C:\msys64\usr\bin;C:\Program Files\Git\usr\bin;$env:PATH"

Write-Host "[env-android] NDK=$ndk"
Write-Host "[env-android] CC=$($env:CC_aarch64_linux_android)"
Write-Host "[env-android] CFLAGS=$($env:CFLAGS) (MSVC flags replaced by the NDK --target)"
Write-Host "[env-android] LINKER=$($env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER)"
Write-Host "[env-android] OPENSSL_* scrubbed (Android build needs --features vendored-openssl)"

# Hand control back to the caller with a sane error policy (see note above).
$ErrorActionPreference = 'Continue'