# Android 交叉编译环境（本机）。dot-source 本文件后再跑 cargo：
#
#   powershell -NoProfile -ExecutionPolicy Bypass -Command ". '.cargo\env-check.ps1'; . '.cargo\env-android.ps1'; cargo check -p copper-core --lib --target aarch64-linux-android"
#
# 为什么需要单独一份（与 env-check.ps1 的分工）：
#   - env-check.ps1 解决的是**桌面 MSVC** 的问题（D8050、OpenSSL 路径、TEMP 重定向），
#     它是桌面构建的唯一事实源；
#   - 本文件解决的是**交叉编译到 aarch64-linux-android** 的问题，两者不能混在一份里：
#     env-check.ps1 设的 CFLAGS = "/MD /O2 /Brepro" 是 MSVC 参数，会被带进 NDK clang
#     的调用里直接失败（实测：sqlite3.c 编译报错、clang 收到 /MD）。
#
# 实测踩过的三个坑（都记在这里，别再重复试）：
#   1. CFLAGS 必须清空，否则 MSVC 参数污染 NDK clang；
#   2. cc-rs 必须能显式找到 NDK 的 clang。只设 PATH 不够：msys 的 clang 会先被选中，
#      它不认 `--target=aarch64-linux-android`。这里直接给出带目标前缀的编译器；
#   3. vendored-openssl 在本机交叉编译仍会失败（OpenSSL Configure 把 CC 切成
#      “目录 + 程序名”后拼成 ...\binclang.exe，少了分隔符）。故安卓类型检查**不带**
#      `--features vendored-openssl`；正式出包请在 CI（Linux runner）上做。

$ErrorActionPreference = 'Stop'

$ndk = 'D:\android\ndk\27.3.13750724'
if (-not (Test-Path $ndk)) {
    Write-Warning "[env-android] 未找到 NDK：$ndk（改这里或安装 r27 LTS）"
}

$tc = "$ndk\toolchains\llvm\prebuilt\windows-x86_64\bin"

$env:ANDROID_HOME     = 'D:\android'
$env:ANDROID_SDK_ROOT = 'D:\android'
$env:NDK_HOME         = $ndk
$env:ANDROID_NDK_ROOT = $ndk
$env:JAVA_HOME        = 'D:\jdk'

# MSVC 参数不能外溢到 NDK clang（见文件头第 1 条）。
$env:CFLAGS = ''

# cc-rs 的查找键用「连字符」形式的目标三元组，不是下划线。
$env:CC_aarch64_linux_android     = "$tc\aarch64-linux-android24-clang.cmd"
$env:CXX_aarch64_linux_android    = "$tc\aarch64-linux-android24-clang++.cmd"
$env:AR_aarch64_linux_android     = "$tc\llvm-ar.exe"
$env:RANLIB_aarch64_linux_android = "$tc\llvm-ranlib.exe"
# 少数 crate 只认下划线形式，一并给上（两者不冲突）。
$env:CC_aarch64_linux_android_underscore = $env:CC_aarch64_linux_android

# perl 用于 vendored OpenSSL（本机在 msys 下），make 亦在此目录。
$env:PATH = "C:\msys64\usr\bin;C:\Program Files\Git\usr\bin;$env:PATH"

Write-Host "[env-android] NDK=$ndk, CC=$($env:CC_aarch64_linux_android), CFLAGS 已清空"
