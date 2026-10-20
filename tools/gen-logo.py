#!/usr/bin/env python3
"""把美术给的图（assets/logo_.jpg）做成应用图标。

产物（都在 assets/ 下）：
  logo.png   512×512 位图（Electron 窗口图标、README 引用）
  logo.ico   多尺寸 Windows 图标（16/24/32/48/64/128/256，electron-builder 用它打 exe 图标）

用法：
  python tools/gen-logo.py

为什么是"转换"而不是"绘制"：
  图形本身是美术交付的黑天鹅线稿（纯黑线条、白底 JPG）。这里只做三件事——
  ① 白底转透明（按亮度取 alpha，黑线保留、JPEG 的灰噪点被压掉）；
  ② 裁到图形实际外框，等比缩放到纸白圆角方块中间（不裁掉翅膀尖，也不贴边）；
  ③ 输出多尺寸 ICO。**图形不要在这里改**：换图就替换 assets/logo_.jpg 后重跑。

设计约定：
  * 底板是纸白圆角方块（#F6F3EC，圆角 22.5%）——线稿是黑的，需要一块浅底才看得清；
    深色任务栏上也靠这块底把轮廓托出来。
  * 图形占底板 76%，四周留白；小尺寸按同一比例缩，不做"小图特调"。
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageOps

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"
SOURCE = ASSETS / "logo_.jpg"

PAPER = (246, 243, 236, 255)
INK = (22, 23, 26, 255)
INK_ALPHA_FLOOR = 14  # 低于这个亮度差当背景：压掉 JPEG 的灰底噪点

MARK_SPAN = 0.70  # 图形占底板边长
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def load_mark() -> Image.Image:
    """读出线稿并把白底变透明：黑色线条保留 alpha，白底 alpha=0。"""
    gray = Image.open(SOURCE).convert("L")
    alpha = ImageOps.invert(gray)  # 黑→255，白→0
    # 压掉接近背景的取值，避免 JPEG 噪点在浅底上形成脏边
    alpha = alpha.point(
        lambda v: 0
        if v <= INK_ALPHA_FLOOR
        else min(255, int((v - INK_ALPHA_FLOOR) * 255 / (255 - INK_ALPHA_FLOOR)))
    )

    bbox = alpha.point(lambda v: 255 if v > 0 else 0).getbbox()
    if bbox is None:
        raise SystemExit(f"没在 {SOURCE} 里找到黑色图形")
    alpha = alpha.crop(bbox)

    mark = Image.new("RGBA", alpha.size, INK[:3] + (0,))
    mark.putalpha(alpha)
    return mark


def rounded_square_mask(size: int, radius_ratio: float = 0.225) -> Image.Image:
    ss = 4
    s = size * ss
    shape = Image.new("L", (s, s), 0)
    ImageDraw.Draw(shape).rounded_rectangle([0, 0, s - 1, s - 1], radius=s * radius_ratio, fill=255)
    return shape.resize((size, size), Image.LANCZOS)


def draw_logo(mark: Image.Image, size: int) -> Image.Image:
    """纸白圆角底 + 居中的天鹅线稿。"""
    tile = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    tile.paste(Image.new("RGBA", (size, size), PAPER), (0, 0), rounded_square_mask(size))

    target = size * MARK_SPAN
    w, h = mark.size
    scale = min(target / w, target / h)
    nw, nh = max(1, round(w * scale)), max(1, round(h * scale))
    tile.alpha_composite(mark.resize((nw, nh), Image.LANCZOS), ((size - nw) // 2, (size - nh) // 2))
    return tile


def main():
    mark = load_mark()
    print(f"source {SOURCE.name}: mark {mark.size[0]}×{mark.size[1]}")

    draw_logo(mark, 512).save(ASSETS / "logo.png")

    frames = [draw_logo(mark, n) for n in ICO_SIZES]
    frames[-1].save(
        ASSETS / "logo.ico",
        format="ICO",
        sizes=[(n, n) for n in ICO_SIZES],
        append_images=frames[:-1],
    )
    print("wrote assets/logo.png, assets/logo.ico")


if __name__ == "__main__":
    main()
