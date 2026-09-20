# Store 授权服务

## 需求

为 Windows Store / MCBE 原生安装链路提供 GPL-3.0-only 的 Store license 协议层。服务动态选择 catalog 中请求产品的零售 MSIXVC ContentID，构造 license 请求，限制 HTTP 响应体，并严格解析 LicenseInfo 与 SPLicenseBlock。WAM、device ticket、用户 ticket 和解包后的内容密钥不得进入命令、事件、日志或持久化边界。

## 架构

`src-tauri/src/services/store_entitlement.rs` 使用现有 `reqwest`、`serde`、`base64`、`quick-xml`，并调用 `msixvc::unpack_content_keys`。catalog 仅接受产品匹配、非 trial/pre-order、MSIXVC、目标 PFN、x64 且 UUID 形状合法的包；catalog 上限为 8 MiB，license 响应上限为 2 MiB，最多 16 条 license 记录和 256 个内容 key。

license 请求体固定包含 ClientChallenge、`concurrencyMode=Rude`、`licenseVersion=4`、`needKey=true`、`keyOnly=true` 及 MSA ticket/reference；device authorization 由调用方放入 Authorization 头。XML 解析要求根元素为 `License`，且恰有一个带严格 `Type=Full|Trial` 的 `LicenseInfo` 和一个 `SPLicenseBlock`；其 Base64 内容交给 MSIXVC TLV 解包。按 KeyID 选择时，大小写不敏感匹配必须唯一，缺失或重复均拒绝。内部 `StoreTicket` 是 opaque 类型，没有公开构造、序列化或克隆能力。

## 备注

实现来源注释保留 Xodus commit `0670e25aeb0e0e9f800f8f2f4968ae3b681842a7`，对应参考代码位于 `libs/LeviLauncher/internal/nativeinstall`，并遵循 GPL-3.0-only。当前没有伪造 WAM/device ticket；`acquire_store_ticket_for_xuid` 委托 `store_wam` 适配器并映射其安全错误，要求显式 expected XUID，并在 Windows 路径通过 `WebAuthenticationCoreManager` 查找 provider/account、请求静默 token、读取实际账户 ID 与 token 后进行绑定校验；非 Windows 返回 `WindowsOnly`。`store_device::provision_device` 已提供 Windows-only 的真实 `deviceaddcredential.srf` 网络编排：随机凭据仅在请求生命周期内生成，设备信息必须由调用方从 Windows 平台采集，响应受限并严格提取 PUID/SPLicenseBlock，绝不生成或返回伪造 ticket。RST 的 XML 签名与 device-ticket 编排已接入 `native_install`。内容 key 只存在短生命周期内部租约中，退出作用域时清理底层密钥；不实现 license 密钥缓存。默认安装流程在包为**含加密区域的 MSIXVC** 时先尝试商店授权：取得密钥则走原生提取，授权不可用（未登录、非 Windows、包为 ZIP、WAM 需要交互、网络失败）则记录原因并回退兼容后端；绝不会伪造密钥。风险点：整条链尚未在真实 Windows 账户与真实加密 MSIXVC 上做过端到端验收，因此 `store_rst_transport` 的 c14n/签名细节、RST 响应解密、设备信息采集与许可证字段仍需实机验证。

`services/native_install.rs` 是整条授权链的统一入口，对应 LeviLauncher `nativeinstall.Install`：读取包身份（XVD 头 `0x220` 的 VDUID 作为 licensing 用的 ContentID，XVC `16..32` 的 KeyID）→ WAM 用户票据（显式 XUID 绑定）→ DPAPI 设备凭据（缺失时用 `GetSystemFirmwareTable('RSMB')` 采集设备信息并真实注册）→ 两阶段 RST 设备票据 → Store content license → SPLicense 解包并按设备绑定校验 → 按 KeyID 选取内容密钥。设备缓存读写、跨进程互斥（`device.lock`，共享模式 0）、市场代码校验和敏感字段清零都在本模块；`acquire_package_content_key` 是唯一对外入口。

`services/store_rst_transport.rs` 提供 `https://login.live.com/RST2.srf` 的 WS-Trust 传输：独占 XML 规范化（c14n 1.0，空前缀列表）、`make_rst` 生成带三处 `Reference` 摘要的 RST 信封、RSA-PKCS1v15(SHA-256) 与 WS-SecureConversation 双重派生 HMAC-SHA256 签名、`post_rst` 有界 POST，以及响应签名校验、`Reference` 摘要校验与 `EncryptedData` 解密。`store_rst.rs` 提供其中复用的 CLEP/AES/RSA 原语。

`services/store_rst.rs` 现在提供跨平台、无网络副作用的 RST 算法组件：CLEP schedule key、无填充 AES-128-CBC 解密、CLEP 解密、device wrapping key 与其 CBC/HMAC 校验、BCrypt `RSA2` 私钥 blob 解析，以及 WS-SecureConversation HMAC-SHA256 derived key。SPLicense TLV 与 owned-device 状态继续复用 MSIXVC parser；Windows DPAPI 继续复用 `store_device`。本次不实现 XML 签名、RST XML 构造/解析或网络请求，避免引入未经验证的协议行为。CLEP 常量和推导实现保留 LukeFZ（MIT SPLicense 工作）归属；整体移植遵循 Xodus/LeviLauncher GPL-3.0-only 归属。

`services/store_wam.rs` 是 LeviLauncher `internal/xbox/wam_windows.go` 与 `auth_windows.go` 的 Windows-only WAM 边界。公开 API 为 `acquire_store_ticket_for_xuid(expected_xuid)`，调用方必须传入 UI 当前显示的 XUID，禁止隐式切换账户。当前使用 `windows = 0.58` 的依赖配置，但由于 WAM 异步 ABI/投影仍需在 Windows SDK 环境中逐项验证，适配器采用明确的安全骨架：Windows 返回 `InteractionRequired`，非 Windows 返回 `WindowsOnly`；两者都不生成或伪造 token。后续接入 `WebAuthenticationCoreManager` 时必须在返回 `StoreTicket` 前验证实际 account reference 与 expected XUID，并将取消、账户切换分别映射为交互/账户错误。