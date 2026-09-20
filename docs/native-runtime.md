# 原生运行库与授权边界

## 需求

XAL 登录运行库和 MSIXVC 兼容解包运行库必须区分身份读取、商店授权和容器提取职责。敏感票据、设备凭据和内容密钥不得进入前端、日志、命令行或普通数据库。

## 架构

当前 `services/xal.rs` 使用独立的 `res/xal` 资源提供本机 XAL 身份读取；`game_download/extractor.rs` 使用 `resources/gdkshared` 的闭源解包 DLL。两者仍存在重复运行库和版本漂移风险。MSIXVC 原生 Rust 模块不应依赖这些 DLL 的私有导出；在 Store 授权链和 ABI 兼容性完成前，旧后端不能贸然删除。

后续应新增独立的 Store entitlement 服务，负责 Store ticket、device ticket、catalog/content ID、license XML/SPLicense 解析和短时 key lease，并绑定当前账户 epoch 与 XUID。安装器只接收内部授权上下文，不向 Tauri 暴露票据或内容密钥。

## 备注

当前未删除 DLL，也未合并两份资源，以避免破坏 XAL 登录。发布前必须完成：唯一资源归属、版本和 SHA-256 清单、签名/来源记录、缓存目录 ACL、失败清理、账户切换并发测试，以及 GPL/第三方许可证审查。原生 MSIXVC 完整提取接入后，才能删除 extractor 的闭源 DLL 分支和对应依赖。DLL 属于独立的非 GPL 组件，不能因 native-install 的 GPL 归属而自动重新许可。
