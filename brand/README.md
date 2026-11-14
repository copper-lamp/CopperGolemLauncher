# brand/ — 品牌图源

本目录只放**生成落地图标用的源图**，所有尺寸图由 `scripts/build-brand-assets.py` 从这些源图产出，不要手工往 `src-tauri/icons`、`gen/android/.../res`、`frontend/public` 里塞图。

## 源图（全部 1024×1024，RGBA）

| 文件 | 来源 | 用途 |
| --- | --- | --- |
| `app-win.png` | `libs/2x/icon_pc.png` | Windows 正式版图标（圆角深棕方块 + 全身铜傀儡） |
| `app-win-preview.png` | `libs/2x/next_icon_pc.png` | Windows 预览版图标（右下 NEXT 角标） |
| `app-android.png` | `libs/2x/icon_pe.png` | Android 正式版图标（留白更大，供系统遮罩） |
| `app-android-preview.png` | `libs/2x/next_icon_pe.png` | Android 预览版图标，当前未接入构建，仅存档 |
| `copper-golem-full.png` | `libs/2x/icon.png` | 应用内品牌位（侧导航 / 加载页 / 引导页 / 发布说明） |
| `copper-golem-head.png` | `libs/2x/head.png` | 只含头部，供 favicon 与小尺寸识别 |

规范化处理：统一裁成 1024²；预览版裁掉底部多余的透明条（`next_*` 原图下方有空白，直接缩放会让主体偏上）。

## 生成

```bash
cd CopperCore
python scripts/build-brand-assets.py
```

依赖 Pillow 与仓库内的 `@tauri-apps/cli`。脚本一次产出：

- `src-tauri/icons/`：`32x32.png`、`128x128.png`、`128x128@2x.png`、`icon.png`、`icon.ico`、`icon.icns` —— 与 `tauri.conf.json` 的 `bundle.icon` 一一对应，多余产物（Square*Logo、ios/、android/ 副产物）会被删掉。
- `src-tauri/icons-preview/`：同一套六件，来自 `app-win-preview.png`，供 `tauri.preview.conf.json` 使用。
- `src-tauri/gen/android/.../res/`：5 档 dpi 的 `ic_launcher.png`、`ic_launcher_round.png`、`ic_launcher_foreground.png`，以及 `mipmap-anydpi-v26/` 自适应图标 XML 与 `values/ic_launcher_background.xml`（`#42210B`）。
- `frontend/public/`：只留 `app-256.png`（全身，26/32/56/96px 共用）与 `favicon.ico`（头部，16/24/32/48/64/256）。

## 约定

- 自适应图标前景 = 头部缩到 108dp 画布的 60%，落在 66.7% 的可见安全区内；API<26 的 legacy PNG 用同一套「纯色底 + 60% 前景」预合成，避免新旧系统头像大小不一致。
- 不再产出没人引用的图。前端曾经同时存在 `app-32/64/180/192/512.png` 五个文件、页面只用其中一张，已全部删除并统一到 `app-256.png`。
- 预览版与正式版**同一 identifier `com.copperlamp.coppergolem`**，不并存安装；差异只有图标与显示名称（`ProductName` = `CopperGolem Preview`，窗口标题「铜傀儡启动器（预览版）」）。构建：`npm run tauri:build:preview`。

## 许可

图标为铜傀儡项目自制资产，版权归项目所有，随仓库一同分发。
