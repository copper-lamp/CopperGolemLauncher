#!/usr/bin/env python3
"""品牌图标生成：从 brand/ 源图一次产出全部落地图标。

为什么要有这个脚本
----------------
图标散落在三处（桌面 src-tauri/icons、Android gen/android res、前端 public），
各自需求不同：桌面要 ico 全家桶、Android 要自适应图标（前景+纯色底）、
前端要的是能在 26–96px 下不糊的 PNG。手工拷文件必然出现「生成了很多张，
实际只引用一张糊图」的情况，所以这里把「源图 → 目标」写成可复现的映射，
一次跑完，并且只产出被代码真正引用的文件。

依赖
----
- Pillow（本机 Python 环境已带）
- 仓库内的 Tauri CLI：node_modules/@tauri-apps/cli/tauri.js（生成 ico/icns 与 Android mipmap）

用法
----
    python scripts/build-brand-assets.py

源图（brand/）
--------------
    app-win.png            正式版 · Windows 图标（圆角深棕方块）
    app-android.png        正式版 · Android 图标（留白更大，给系统遮罩）
    app-win-preview.png    预览版 · Windows 图标（右下 NEXT 角标）
    app-android-preview.png 预览版 · Android 图标
    copper-golem-full.png  应用内品牌位（侧导航 / 加载页 / 发布说明）
    copper-golem-head.png  小尺寸位（favicon）
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageColor, ImageDraw

ROOT = Path(__file__).resolve().parent.parent          # CopperCore/
BRAND = ROOT / "brand"
TAURI_CLI = ROOT / "node_modules" / "@tauri-apps" / "cli" / "tauri.js"

DESKTOP_ICONS = ROOT / "src-tauri" / "icons"
DESKTOP_ICONS_PREVIEW = ROOT / "src-tauri" / "icons-preview"
ANDROID_RES = ROOT / "src-tauri" / "gen" / "android" / "app" / "src" / "main" / "res"
PUBLIC = ROOT / "frontend" / "public"

# 桌面端真正被 tauri.conf.json 引用的 6 个文件；其余（Square*Logo、ios/、android/）
# 是 tauri icon 的副产品，不参与 Windows/Android 打包，直接丢弃。
DESKTOP_KEEP = ["32x32.png", "128x128.png", "128x128@2x.png", "icon.png", "icon.ico", "icon.icns"]

# 与 app-android.png 里的方块底色一致，供自适应图标背景层使用。
ICON_BACKGROUND = "#42210B"
# 自适应图标前景：头部内容高度占 108dp 画布的 60%，落在 66.7% 的圆形可见区内。
FOREGROUND_CONTENT_RATIO = 0.60


def run_tauri_icon(source: Path, output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    cmd = ["node", str(TAURI_CLI), "icon", str(source), "-o", str(output)]
    proc = subprocess.run(cmd, cwd=str(ROOT), capture_output=True, text=True)
    if proc.returncode != 0:
        sys.stderr.write(proc.stdout + "\n" + proc.stderr + "\n")
        raise SystemExit(f"tauri icon 失败，退出码 {proc.returncode}")


def copy_desktop_icons(generated: Path, target: Path) -> None:
    target.mkdir(parents=True, exist_ok=True)
    for name in DESKTOP_KEEP:
        src = generated / name
        if not src.is_file():
            raise SystemExit(f"缺失 {src}，tauri icon 输出与预期不符")
        shutil.copyfile(src, target / name)
    # 清掉旧版本残留（Square*Logo 为 MSIX 专用，本项目无 MSIX target）。
    for stale in target.iterdir():
        if stale.is_file() and stale.name not in DESKTOP_KEEP:
            stale.unlink()
    print(f"[desktop] {target.relative_to(ROOT)}: {', '.join(DESKTOP_KEEP)}")


def android_foreground(workdir: Path) -> Path:
    """把头部图按安全区缩进一块透明画布，作为自适应图标的前景层。

    Android 会把前景层按 108dp 画布绘制、再用形状遮罩裁到中间 72dp，
    所以前景必须自己留白；直接丢整张 1024 的头部图会被裁掉耳朵。
    """
    head = Image.open(BRAND / "copper-golem-head.png").convert("RGBA")
    bbox = head.getchannel("A").getbbox()
    if bbox is None:
        raise SystemExit("copper-golem-head.png 全透明，无法作为前景")
    content = head.crop(bbox)

    canvas_size = 1024
    target_h = int(canvas_size * FOREGROUND_CONTENT_RATIO)
    scale = target_h / content.height
    resized = content.resize((max(1, round(content.width * scale)), target_h), Image.LANCZOS)

    canvas = Image.new("RGBA", (canvas_size, canvas_size), (0, 0, 0, 0))
    canvas.alpha_composite(
        resized,
        ((canvas_size - resized.width) // 2, (canvas_size - resized.height) // 2),
    )
    out = workdir / "android-foreground.png"
    canvas.save(out)
    return out


LEGACY_SIZES = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
FOREGROUND_SIZES = {"mdpi": 108, "hdpi": 162, "xhdpi": 216, "xxhdpi": 324, "xxxhdpi": 432}

ADAPTIVE_ICON_XML = """<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
  <background android:drawable="@color/ic_launcher_background"/>
  <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
</adaptive-icon>
"""


def rounded(image: Image.Image, radius_ratio: float = 0.20) -> Image.Image:
    """按比例圆角裁切，和桌面图标保持同一种观感。"""
    size = image.size[0]
    radius = max(1, round(size * radius_ratio))
    scale = 4
    mask = Image.new("L", (size * scale, size * scale), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, size * scale - 1, size * scale - 1), radius=radius * scale, fill=255
    )
    mask = mask.resize((size, size), Image.LANCZOS)
    out = image.copy()
    out.putalpha(Image.composite(image.getchannel("A"), Image.new("L", (size, size), 0), mask))
    return out


def circular(image: Image.Image) -> Image.Image:
    """把方形图标裁成圆形，供 API<26 的圆形启动器使用。

    Android 8 起走 mipmap-anydpi-v26 的自适应图标，轮不到这两个 PNG；
    旧系统不会替我们裁圆，所以这里直接给圆形的成品。
    """
    size = image.size[0]
    mask = Image.new("L", (size * 4, size * 4), 0)
    ImageDraw.Draw(mask).ellipse((0, 0, size * 4 - 1, size * 4 - 1), fill=255)
    mask = mask.resize((size, size), Image.LANCZOS)
    out = image.copy()
    out.putalpha(Image.composite(image.getchannel("A"), Image.new("L", (size, size), 0), mask))
    return out


def build_android(workdir: Path) -> None:
    """手写 Android 资源：tauri icon 的 manifest 只支持「背景图」不支持纯色底，
    而自适应图标的背景必须是一块纯色（否则遮罩边缘会露出贴图的接缝）。"""
    foreground = Image.open(android_foreground(workdir)).convert("RGBA")

    # API<26 的旧系统直接用这两张 PNG，不会自己套自适应图标，所以这里预先合成
    # 「纯色底 + 60% 前景」，与 ≥26 的自适应结果在视觉上完全一致；否则同一台
    # 手机上换一个系统版本，头像大小就变了。
    master = Image.new("RGBA", foreground.size, (*ImageColor.getrgb(ICON_BACKGROUND), 255))
    master.alpha_composite(foreground)
    master = rounded(master)

    for density, size in LEGACY_SIZES.items():
        dst = ANDROID_RES / f"mipmap-{density}"
        dst.mkdir(parents=True, exist_ok=True)
        legacy = master.resize((size, size), Image.LANCZOS)
        legacy.save(dst / "ic_launcher.png", optimize=True)
        circular(legacy).save(dst / "ic_launcher_round.png", optimize=True)

        fg_size = FOREGROUND_SIZES[density]
        foreground.resize((fg_size, fg_size), Image.LANCZOS).save(
            dst / "ic_launcher_foreground.png", optimize=True
        )

    anydpi = ANDROID_RES / "mipmap-anydpi-v26"
    anydpi.mkdir(parents=True, exist_ok=True)
    (anydpi / "ic_launcher.xml").write_text(ADAPTIVE_ICON_XML, encoding="utf-8")
    (anydpi / "ic_launcher_round.xml").write_text(ADAPTIVE_ICON_XML, encoding="utf-8")

    values = ANDROID_RES / "values"
    values.mkdir(parents=True, exist_ok=True)
    (values / "ic_launcher_background.xml").write_text(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        "<resources>\n"
        f'  <color name="ic_launcher_background">{ICON_BACKGROUND}</color>\n'
        "</resources>\n",
        encoding="utf-8",
    )

    # 模板残留：自适应图标改用 mipmap 前景 + 纯色底后，这两个 vector 不再被任何
    # 清单引用（AndroidManifest 只引用 @mipmap/ic_launcher）。
    for stale in [
        ANDROID_RES / "drawable" / "ic_launcher_background.xml",
        ANDROID_RES / "drawable-v24" / "ic_launcher_foreground.xml",
    ]:
        if stale.is_file():
            stale.unlink()
    print(f"[android] {ANDROID_RES.relative_to(ROOT)}: mipmap-*(5) + mipmap-anydpi-v26")


def build_frontend_public() -> None:
    """前端只保留两个真正被引用的品牌图。

    app-256.png：侧导航 26px、加载页 56px、引导页 32px、发布说明 96px 共用，
                 这些尺寸在 1.5x/2x 缩放下仍有余量，不会再出现糊图。
    favicon.ico：浏览器标签页（开发期）与 PWA 兜底，16/32/48 三档。
    """
    full = Image.open(BRAND / "copper-golem-full.png").convert("RGBA")
    full.resize((256, 256), Image.LANCZOS).save(PUBLIC / "app-256.png", optimize=True)

    head = Image.open(BRAND / "copper-golem-head.png").convert("RGBA")
    head.resize((48, 48), Image.LANCZOS).save(
        PUBLIC / "favicon.ico",
        sizes=[(16, 16), (32, 32), (48, 48)],
    )

    for stale in ["app-32.png", "app-64.png", "app-180.png", "app-192.png", "app-512.png"]:
        path = PUBLIC / stale
        if path.is_file():
            path.unlink()
    print(f"[frontend] {PUBLIC.relative_to(ROOT)}: app-256.png, favicon.ico")


def main() -> None:
    if not TAURI_CLI.is_file():
        raise SystemExit(f"未找到 Tauri CLI：{TAURI_CLI}（先在 CopperCore 执行 npm install）")
    for name in [
        "app-win.png",
        "app-android.png",
        "app-win-preview.png",
        "copper-golem-full.png",
        "copper-golem-head.png",
    ]:
        if not (BRAND / name).is_file():
            raise SystemExit(f"缺少源图 brand/{name}")

    with tempfile.TemporaryDirectory(prefix="cgl-brand-") as tmp:
        workdir = Path(tmp)
        release_out = workdir / "release"
        run_tauri_icon(BRAND / "app-win.png", release_out)
        copy_desktop_icons(release_out, DESKTOP_ICONS)

        preview_out = workdir / "preview"
        run_tauri_icon(BRAND / "app-win-preview.png", preview_out)
        copy_desktop_icons(preview_out, DESKTOP_ICONS_PREVIEW)

        build_android(workdir)

    build_frontend_public()
    print("完成")


if __name__ == "__main__":
    main()
