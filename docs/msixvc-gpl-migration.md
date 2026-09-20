# MSIXVC GPL 移植合规

## 需求

用户已明确允许按 GPL-3.0-only 合规要求移植 LeviLauncher 的 `nativeinstall`。因此后续 Rust 移植可以直接参考并改写其实现，但不能删除版权、许可证和来源信息，也不能声称该部分是无来源的独立原创代码。

## 架构

移植范围包括 XVD/MSIXVC 解析、哈希树、AES-XTS、SPLicense、device ticket、RST、license、Store entitlement 及必要的 Xbox 适配。CopperCore 现有 `AccountService`、`xal.rs`、installer 和事件系统作为宿主接口；敏感 token、device credential 和 content key 只允许后端内部流转。Windows 适配预留了 `windows` crate 的 WinRT/WAM、DPAPI、文件锁和系统信息 features，但这些 features 不代表协议已经实现。

移植后的 nativeinstall 代码应保留 GPL-3.0-only 文件头或等效目录级声明，并在根目录 `THIRD_PARTY_NOTICES` 记录 LeviLauncher 来源、许可证和源码披露义务。CopperCore 根 `LICENSE` 已是 GPL 系列许可证；不得主动改版本号。

## 备注

当前工作树已经加入合规说明、`extract_package_with_key` 的敏感 key 入口，以及 `store_device`、`store_entitlement`、`store_rst`、`store_wam` 的逐文件来源标记。WAM/device ticket 的真实 Windows 协议仍未完成，故尚未接入默认 installer 或删除旧 DLL；只有在真实授权环境和真实 MSIXVC 包验收后才可移除兼容后端。发布归档必须提供对应 CopperCore 源码、Cargo.lock、构建脚本、资源生成输入和本通知。
