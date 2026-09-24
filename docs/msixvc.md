# MSIXVC 原生安装

## 需求

游戏下载模块需要在 Windows x64 上识别并安全处理 MSIXVC/XVD 容器，同时保留历史 `.appx` ZIP 回退。输入包必须保持只读；安装失败不得留下可被误认为已安装的半成品目录。MSIXVC 的商店授权、设备绑定和内容密钥不能进入前端、日志或普通数据库。

## 架构

`src-tauri/src/modules/game_download/msixvc.rs` 负责独立实现的格式边界：XVD 头部约束、User Data/SegmentMetadata 解析、文件大小边界、UTF-16 解码、路径词法安全、大小写冲突与文件/目录冲突检测，以及哈希树、RFC 3394 解包和 AES-XTS 页密码学原语。

哈希树使用 4 KiB 页面、每节点 170 个子摘要、24 字节槽位和 SHA-256 前 20 字节；根摘要来自头部。提取时会验证根、所有分支和 User Data/XVC 数据页。所有输入范围必须先做 checked arithmetic，并在读取前验证文件截断。

`extract_xvc` 接受短时借用的 32 字节 content key；`ContentKeyLease` 在离开作用域时清零。`extract_package_with_key` 是 installer/授权服务未来接入点，key 不会经过前端、JSON、日志或命令行。

授权链尚未接入：现有 `AccountService` 提供 MSA/Xbox/XSTS 身份，但不提供 Store ticket、device ticket 或许可证内容密钥。`xal.rs` 读取本机 XAL 身份也不能替代 Store 授权。因此 installer 当前仍通过旧 extractor 后端处理无 key 的加密 MSIXVC，不能宣称已完成纯 Rust 授权闭环。

## 备注

当前已完成：头校验（包括保留区、动态区和大小上限）、解析器测试、路径安全 API、哈希树/RFC3394/AES-XTS 原语、受限 SPLicense TLV 与设备绑定内容密钥记录解析，以及带 staging/原子发布的 XVC 区域提取核心。待完成：Store 授权服务和 installer 到 content key 的闭环、真实包端到端验收，以及旧私有 DLL 清理。

安全风险：路径必须拒绝绝对路径、盘符、UNC、控制字符、保留设备名、尾部空格/点、`.`/`..` 和大小写碰撞；授权失败时保留下载包并清除半成品目录。不得复制 GPL 实现源码，只能依据格式/密码学行为独立实现；发布前需要完成许可证审查和真实包验收。
