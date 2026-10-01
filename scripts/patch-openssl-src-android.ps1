# Copper Golem: patch `openssl-src` so the vendored OpenSSL build works when
# cross-compiling to Android from a Windows host.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\patch-openssl-src-android.ps1
#
# Why this is needed
# ------------------
# `openssl-src` asks cc-rs for the cross C compiler and hands the answer to
# OpenSSL's `Configure` as the `CC`/`AR`/`RANLIB` environment variables. On a
# Windows host cc-rs reports a path with Windows separators, e.g.
#
#   D:/android/ndk/27.3.13750724/toolchains/llvm/prebuilt/windows-x86_64/bin\clang.exe
#
# OpenSSL's build then runs the generated Makefile through a POSIX shell (`sh`
# from Git for Windows / msys2), which eats that backslash, so `make` tries to
# run `.../windows-x86_64/binclang.exe` and fails with
#
#   /bin/sh: line 1: .../binclang.exe: No such file or directory
#   make[1]: *** [Makefile:4432: crypto/aes/libcrypto-lib-aes-sha1-armv8.o] Error 127
#
# The patch replaces backslashes with forward slashes for those three variables,
# which is exactly how `openssl-src` already treats `--prefix` (`sanitize_sh`).
# It sits inside the `if !target.contains("msvc")` branch, so MSVC builds are
# untouched. This is an upstream portability defect; consider reporting it.
#
# Where it is applied
# -------------------
# Into the unpacked cargo registry, because cargo unpacks crates there. The
# script looks at the repository-local CARGO_HOME first (`.cargohome`, which is
# what `scripts/build-android-apk.cmd` uses), then falls back to `%CARGO_HOME%`
# or `%USERPROFILE%\.cargo`.
#
# Re-run it after a `cargo update` pulls a different `openssl-src` version, or on
# a fresh machine. The script is idempotent.

$ErrorActionPreference = 'Stop'

$crateName = 'openssl-src-300.6.1+3.6.3'
$repoRoot = Split-Path $PSScriptRoot -Parent
$patch = Join-Path $repoRoot 'patches\openssl-src-android-sh-paths.patch'

if (-not (Test-Path $patch)) {
    Write-Error "[openssl-src] patch file not found: $patch"
}

$fromEnv = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { $null }
$candidates = @(
    (Join-Path $repoRoot '.cargohome'),
    $fromEnv,
    (Join-Path $env:USERPROFILE '.cargo')
) | Where-Object { $_ }

$target = $null
foreach ($cargoHome in $candidates) {
    $srcRoots = Get-ChildItem (Join-Path $cargoHome 'registry\src') -Directory -ErrorAction SilentlyContinue
    foreach ($root in $srcRoots) {
        $candidate = Join-Path $root.FullName $crateName
        if (Test-Path $candidate) {
            $target = $candidate
            break
        }
    }
    if ($target) { break }
}

if (-not $target) {
    Write-Warning "[openssl-src] $crateName is not unpacked in any known cargo home:"
    $candidates | ForEach-Object { Write-Warning "  - $_" }
    Write-Warning "[openssl-src] Build once so cargo unpacks it (scripts\build-android-apk.cmd), then re-run this script."
    exit 1
}

$lib = Join-Path $target 'src\lib.rs'
if ((Get-Content $lib -Raw) -match 'sh_path') {
    Write-Host "[openssl-src] already patched: $lib"
    exit 0
}

Push-Location $target
try {
    & git apply --verbose --whitespace=nowarn $patch
    if ($LASTEXITCODE -ne 0) {
        Write-Error "[openssl-src] git apply failed (exit $LASTEXITCODE). The crate version probably changed; refresh patches\openssl-src-android-sh-paths.patch."
    }
} finally {
    Pop-Location
}

Write-Host "[openssl-src] patched: $lib"
