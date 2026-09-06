<div align="center">

  <img src="https://trae-api-cn.mchost.guru/api/ide/v1/text_to_image?prompt=modern%20Minecraft%20Bedrock%20game%20launcher%20desktop%20app%20dark%20glassmorphism%20UI%20with%20left%20navigation%20bar%20and%20content%20cards%20for%20game%20versions%20and%20mods%20clean%20futuristic%20design%20high%20detail&image_size=landscape_16_9" alt="铜傀儡启动器界面" width="880">

  <h1>铜傀儡 · CopperGolem</h1>
  <p><strong>下载游戏、管理账号、畅装内容——一次抵达。</strong></p>
  <p>为 Minecraft Bedrock 打造的现代化模块化启动器：把版本、账户与游戏内容，收进一个干净、迅捷的入口。</p>

  <p>
    <img src="https://img.shields.io/badge/release-0.1.0-4c8bf5?style=flat-square" alt="release v0.1.0">
    <img src="https://img.shields.io/badge/Minecraft%20Bedrock-Windows%20x64-62b47a?style=flat-square" alt="Windows x64 Minecraft Bedrock">
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square" alt="GPL-3.0 许可证"></a>
  </p>

  <p>
    <a href="README.en.md">English</a>
    ·
    <a href="#快速开始">快速开始</a>
    ·
    <a href="https://github.com/copper-lamp/CopperGolemLauncher/releases">发行版本</a>
    ·
    <a href="CHANGE.md">更新日志</a>
    ·
    <a href="https://github.com/copper-lamp/CopperGolemLauncher/issues">问题反馈</a>
    ·
    <a href=".github/CONTRIBUTING.md">参与贡献</a>
  </p>

</div>

> [!WARNING]
> 铜傀儡目前处于早期开发阶段，公开版本均为测试版本。请在使用前备份重要世界与数据；当 Minecraft 或启动器自身更新后，不保证旧配置完全兼容。

铜傀儡是一款面向 Minecraft Bedrock 的现代化启动器。它把**版本下载、账号登录与游戏内容**统一到一个干净、迅捷的入口——你花在「准备」上的时间更少，花在「游玩」上的时间更多。

## 快速开始

> [!IMPORTANT]
> 建议从光鲜的官方来源下载，并从干净的环境开始。

1. 从 [发行版本](https://github.com/copper-lamp/CopperGolemLauncher/releases) 下载最新安装包。
2. 安装并启动铜傀儡。
3. 登录你的 Minecraft Bedrock 正版账号。
4. 下载游戏版本与想要的内容，然后点击启动，开玩。

## 为什么选铜傀儡

- **一站管理** — 游戏版本、账号、启动，一个入口全搞定，不再来回倒腾。
- **内容中枢** — 地图、光影、资源、模组，逛逛就能装，装好即玩。
- **轻巧迅捷** — 启动快、占用低，把宝贵的资源留给游戏本身。
- **正版体验** — 原生支持 Minecraft Bedrock 正版账号登录。
- **开放可扩展** — 模块化架构，需要什么装上即用，社区可共建、可生长。
- **以人为本** — 细节打磨到位，界面主题随夜幕与日光自然切换。

## 为谁而生

- **玩家** — 下载游戏、登录账号、一键开玩，轻松管理多个版本与内容。
- **内容作者** — 在统一的内容中心分发作品，触达更广的玩家。
- **开发者** — 基于开放的模块体系扩展启动器，无需改动内核。

## 版本与兼容性

铜傀儡遵循语义化版本约定，当前发布状态如下。

| 平台 | 版本 | 状态 |
| --- | --- | --- |
| Windows x64 · Minecraft Bedrock（基岩版） | `0.1.0` | 早期测试版 |

> [!NOTE]
> 具体变更见 [更新日志](CHANGE.md)。发布自动打包生成，见仓库工作流的 `release` 说明。

## 常见问题

### 铜傀儡是什么？

铜傀儡是一款可扩展、模块化的 Minecraft Bedrock（基岩版）启动器。它把版本下载、正版账号登录与游戏内容管理整合到一个干净、高效的桌面入口中。

### 需要正版账号吗？

支持原生登录 Minecraft Bedrock 正版账号，以获得完整联机与内容体验。

### 可以安装地图、光影、资源、模组吗？

可以。铜傀儡内置**内容中心**模块，可浏览、下载并安装游戏内容；模块化架构也允许按需扩展更多内容来源。

### 会支持其他平台吗？

当前面向 Windows x64 的基岩版。内核在架构上做了平台抽象，为后续更多平台预留了边界。

### 从哪里下载最安全？

始终从本仓库的 [发行版本](https://github.com/copper-lamp/CopperGolemLauncher/releases) 或公告渠道获取，避免使用不明来源的分发包。

## 开发状态与计划

- **进行中** — 铜核心能力服务与模块化桌面接口。
- **规划** — 下载引擎、内容中心，以及更多内置与附加模块。

> [!TIP]
> **测试重点：** 报告问题时，请附上操作系统版本、启动器版本、复现步骤与相关日志，能极大加快定位。

## 已知限制

- 项目处于早期阶段，界面与能力仍在积极开发，不代表最终形态。
- 目前仅支持 Windows x64 平台的基岩版；其他平台支持尚不可用。
- 部分能力（如更完整的账户与会话管理）仍在建设中。
- 不同的 Minecraft 或系统更新后，可能需要重新确认兼容性。

如需报告可复现问题，请[创建 Issue](https://github.com/copper-lamp/CopperGolemLauncher/issues)。

## 参与贡献

我们非常欢迎社区一同塑造铜傀儡。构建与开发环境说明见 [CONTRIBUTING](.github/CONTRIBUTING.md)，可了解分支、Pull Request 与提交规范。

请阅读并遵守 [行为准则](.github/CODE_OF_CONDUCT.md)。参与本项目即表示你同意其条款。安全问题请按 [SECURITY](.github/SECURITY.md) 私下报告，**不要**为安全漏洞创建公开 Issue。

## 行为准则

铜傀儡采用 Contributor Covenant 行为准则。请在参与 Issue、Pull Request 或讨论之前阅读 [CODE_OF_CONDUCT.md](.github/CODE_OF_CONDUCT.md)。

## 致谢

感谢 Minecraft Bedrock 玩家社区与开源生态，以及我们深入参考的开源项目（如 LeviLauncher）及其社区——它们的工程经验为铜傀儡的实现提供了宝贵帮助。

## 许可证

Copyright (C) 2026 [copper-lamp](https://github.com/copper-lamp)

铜傀儡采用 [GNU GPL v3.0](LICENSE) 发布。分发修改版本时必须继续使用 GPL-3.0，并提供对应源代码；第三方组件保留各自许可证。