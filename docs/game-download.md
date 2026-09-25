# 游戏下载模块

## 需求

模块负责版本清单、断点下载、MD5 自验、安装状态持久化和完成事件。安装目录只在完整提取及元数据写入后对外可见；下载包在授权失败或提取失败时保留，半成品目录清理。

## 架构

`installer.rs` 以数据库任务记录驱动 `downloading`、`extracting`、`installed`、`failed` 状态，并复用全局下载服务。底层下载引擎将数据流式写入 `.part`：HTTP 200（服务端不支持/忽略 Range）用截断方式从头下载，HTTP 206 用追加方式续传；Windows 下如已有临时文件带只读属性，引擎仅清除该临时文件的只读属性后继续下载；本地文件错误与网络错误分开报告，并包含操作及路径上下文。`.appx` 走 ZIP 回退；`.msixvc` 先执行独立 XVC 头、哈希树、区域表、路径和原子提取校验，缺少 Store content key 时才进入兼容后端。完成后写入版本元数据并广播 `version.installed` 与模块事件，前端通过 i18n 键呈现状态。

## 备注

安装入口已接入商店授权：`finish_install` 在解包前调用 `native_install::requires_store_key` 判断包是否为含加密区域的 MSIXVC，是则先完成商店授权链取得 content key，再以 `Some(key)` 调用 `finish_install_with_key`；授权不可用时以 `None` 回退兼容后端并记录原因。`Ctx` 因此新增 `account`（取当前 XUID）与 `store_http`（40 秒超时、不跟随重定向，避免票据被转发到非目标主机）两个字段。市场代码取系统区域设置中的两位大写地区，非法时回退 `US`。密钥只在一次提取调用期间被借用，不进入任务记录、数据库、事件或错误文本。风险点：真实 Store 账户 + 真实加密 MSIXVC 的端到端验收尚未完成，旧私有 DLL 未删除。


已实现下载断点恢复、流式 MD5、单飞安装锁、XVC staging/原子发布、提取失败清理和元数据失败清理。`finish_install_with_key` 与 `extract_package_with_key` 在入口处拒绝非 32 字节 key。授权链已由 `services/native_install.rs` 提供（WAM → 设备凭据 → 两阶段 RST 设备票据 → content license → SPLicense → KeyID 选键），默认安装入口会为加密 MSIXVC 主动走该链。仍未闭环的是**实机验收**：需要真实 Windows Store 账户、可用的 WAM 交互、真实设备注册与真实加密 MSIXVC 包才能证明整条链在真机成立；在这些证据到齐前不得宣称完整原生安装完成，也不得删除旧兼容 DLL。不得主动修改版本号、CHANGE.md 或 README。
