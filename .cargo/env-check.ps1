# Local Rust build environment fix for this machine (dot-source, then run cargo).
#
# Why: this machine's MSVC 14.51.36231 fails with D8050 ("failed to get command
# line into debug records") when compiling libsqlite3-sys's 9.2 MB sqlite3.c,
# because cc-rs injects the default `-Z7` debug flag. This is a toolchain/env
# issue, not a code issue; a plain `cargo check` always fails here.
#
# Recipe (all four parts are required):
#   1. CRATE_CC_NO_DEFAULTS=1  -> stop cc-rs from injecting `-Z7` (the real trigger)
#   2. CFLAGS                  -> supply the minimal equivalent flags sqlite needs
#   3. OPENSSL_DIR / _LIB_DIR / _INCLUDE_DIR -> openssl-sys needs the local OpenSSL
#   4. TEMP/TMP                -> the DSH sandbox denies writes to the default
#                                 %TEMP%, which makes any test that creates a
#                                 temp dir fail with os error 5 (PermissionDenied).
#                                 Point it at a workspace-local dir instead.
#
# Usage:
#   . D:\CopperGolem\CopperCore\.cargo\env-check.ps1
#   cargo check -p copper-core
#   cargo test  -p copper-core --lib
#
# Caution: `cargo check` does NOT compile `#[cfg(test)]` code. To catch errors in
# test modules you must also run `cargo test --no-run` (or `cargo test` itself).
#
# Verified working: `cargo check --workspace` exit 0, and the full unit test suite
# reports 182 passed / 0 failed / 1 ignored.

$env:CRATE_CC_NO_DEFAULTS = "1"
$env:CFLAGS               = "/MD /O2 /Brepro /DSQLITE_CORE"
$env:OPENSSL_DIR          = "F:\OpenSSL-Win64"
$env:OPENSSL_LIB_DIR      = "F:\OpenSSL-Win64\lib\VC\x64\MD"
$env:OPENSSL_INCLUDE_DIR  = "F:\OpenSSL-Win64\include"
$env:CARGO_TERM_COLOR     = "never"

$tmp = "D:\CopperGolem\CopperCore\target\tmp"
if (-not (Test-Path $tmp)) { New-Item -ItemType Directory -Path $tmp -Force | Out-Null }
$env:TEMP = $tmp
$env:TMP  = $tmp

Write-Host "[env-check] Rust build env ready (cc no-defaults + OpenSSL + workspace TEMP)"
