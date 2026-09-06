# 铜傀儡启动器 · CopperCore

**铜傀儡（CopperGolem）** 是一款可插拔、模块化的 Minecraft Bedrock Edition（MCBE）启动器。
**CopperCore** 是它的内核，也是启动器的骨架与宿主：负责模块加载、能力服务、Shell UI 与模块间的联动中介。

> 项目处于早期开发阶段。本仓库为铜内核（CopperCore），启动器本体内置**开始页**、**游戏下载**、**内容下载**三个模块，附加模块（Agent / MCP）动态加载。

---

## 特性

- **模块化内核**：内置 / 附加模块统一为"前后端同构插件"，可插拔、互不影响，一切协同经内核中介完成。
- **能力服务**：i18n 多语言、主题令牌、SQLite 持久化、下载队列、MCBE 账户、软件更新、文件系统抽象。
- **联动中介**：事件总线（广播式）+ 意图注册表（请求 / 响应式）完成模块间解耦协同。
- **Shell UI**：自定义无边框标题栏、左导航、内容区、设置页、下载悬浮窗。
- **平台抽象**：文件系统 / 下载 / 存储能力不绑定 Windows，为后续 Android 适配预留边界。

## 技术栈

| 层 | 选型 |
|---|---|
| 桌面骨架 | Tauri v2（Rust 后端 + 系统 WebView） |
| 后端语言 | Rust（tokio 异步运行时） |
| 前端框架 | Vue 3 + TypeScript + Vite |
| 持久化 | SQLite（rusqlite 封装，版本迁移） |
| 下载引擎 | 独立 Rust crate `copper-downloader` |

## 架构

铜傀儡由单一桌面进程承载，逻辑分四层：

1. **铜内核（CopperCore）**：唯一的 Tauri App，基础设施与宿主。
2. **能力服务层**：全局唯一设施——i18n、主题令牌、数据库、下载队列、账户、更新、文件系统抽象。
3. **插件注册表**（含事件总线 / 意图注册表）：模块装载与联动中介。
4. **模块层**：内置 3 模块静态编译进内核；附加模块动态加载。

详细设计参见项目 `docs/` 下的《架构总览》《铜核心·设计》与各模块设计文档。

## 目录结构

```
CopperCore/
├─ frontend/        # Vue 3 Shell + 内置模块前端包
├─ src-tauri/       # Rust 后端（services / registry / modules 挂载点在此）
│   ├─ src/
│   ├─ capabilities/
│   ├─ icons/
│   ├─ Cargo.toml
│   └─ tauri.conf.json
├─ .github/workflows/   # build / release 工作流
├─ LICENSE               # GPL-3.0
└─ package.json          # 根入口（转发到 frontend，托管 tauri CLI）
```

## 快速开始（开发）

前置要求：Node.js ≥ 20、Rust stable 工具链、Windows 10 / 11。

```bash
cd CopperCore
npm install                    # 安装根依赖（含 tauri CLI）
npm --prefix frontend install  # 安装前端依赖
```

| 命令 | 说明 |
|---|---|
| `npm run dev` | 仅启动前端 Vite（HMR，端口 1420） |
| `npm run tauri:dev` | 启动完整应用开发（前端 + Rust） |
| `npm run build` | 仅构建前端 |
| `npm run tauri:build` | 构建完整应用 |
| `cargo build --release --manifest-path src-tauri/Cargo.toml` | 仅构建 Rust 后端（release） |

测试：

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # Rust 单元 / 集成测试
```

> 完整构建与测试链路由 CI（build 工作流）统一执行，详见「持续集成」。

## 持续集成（GitHub Actions）

- **`build` 工作流**：提交 / 拉取请求时触发——安装前端依赖、构建前端、构建 Rust（release），持续验证可构建性。
- **`release` 工作流**：发布版本时触发——两种方式：
  - **自动**：推送形如 `v*` 的 tag；
  - **手动**：在 Actions 页面手动运行并填写版本 tag。
  通过 `tauri-action` 打包 Windows 安装包并创建 GitHub Release。

版本号遵循语义化约定，见 [CHANGE.md](./CHANGE.md)。

## i18n

多语言机制由内核能力服务提供；模块以 `module.<模块名>.*` 命名空间合并语言包，文案缺失时回退 `en`，再缺失返回键名。开发模块时请勿硬编码文案。

## 主题令牌

统一 CSS 设计令牌 `var(--copper-*)`，支持 dark / light / auto，切换全局实时生效。开发界面时请消费令牌，勿硬编码颜色与间距，避免风格割裂。

## 许可

本程序以 [GPL-3.0](./LICENSE) 许可发布。
copyright © 2026 copper-lamp。

## 文档

架构与各模块设计文档维护于项目根目录 `docs/`（开发计划、架构总览、铜核心设计、内置与附加模块设计）。详见各文档的《需求 / 架构 / 备注》章节。