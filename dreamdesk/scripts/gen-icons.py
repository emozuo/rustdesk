"""生成 DreamDesk 的程序图标（几何和 dreamdesk/res/icon.svg 一致：深色圆角底 + 首页的菱形标识）。

改了图标就跑一次：python dreamdesk/scripts/gen-icons.py（要 Pillow），把生成的文件一起提交。生成：
- flutter/assets/icon.png、icon.ico：RustDesk 给定制客户端的入口，托盘、界面里的小图标用 png，Windows 快捷方式、
  「程序和功能」用 ico（src/tray.rs load_icon_from_asset、src/platform/windows.rs get_custom_icon）
- flutter/windows/runner/resources/app_icon.ico：程序文件内嵌的图标（任务栏、窗口左上角、资源管理器）
- dreamdesk/res/icon-256.png：Linux 包装进 hicolor 主题的图标（dreamdesk/scripts/build-linux.sh）
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[2]
BG = (5, 5, 10, 255)
RED = (255, 42, 61, 255)
SS = 8  # 先按 8 倍画再缩小，边缘平滑


def render(size: int) -> Image.Image:
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, s - 1, s - 1), radius=round(s * 56 / 256), fill=255)
    bg = Image.new("RGBA", (s, s), BG)
    if size >= 48:
        # 底部偏红的一团光（和首页背景一样），小尺寸上只会糊，不画
        glow = Image.new("RGBA", (s, s), (0, 0, 0, 0))
        ImageDraw.Draw(glow).ellipse((-s * 0.25, s * 0.25, s * 1.25, s * 1.75), fill=(255, 42, 61, 82))
        bg = Image.alpha_composite(bg, glow.filter(ImageFilter.GaussianBlur(s * 0.18)))
    img.paste(bg, (0, 0), mask)
    d = ImageDraw.Draw(img)
    c = s / 2
    # 外框半对角线、描边宽度、实心芯：照首页 _Diamond（描边 = 边长的 7.5%，芯缩进 1/4）；16、32 像素放大一点、描边至少 1 像素
    r = s * (0.40 if size <= 32 else 0.34)
    w = max(r * 0.106, SS * 1.0)
    pts = [(c, c - r), (c + r, c), (c, c + r), (c - r, c)]
    d.polygon(pts, outline=RED, width=round(w))
    ri = r / 2
    d.polygon([(c, c - ri), (c + ri, c), (c, c + ri), (c - ri, c)], fill=RED)
    return img.resize((size, size), Image.LANCZOS)


def save_ico(path: Path, sizes):
    imgs = [render(n) for n in sizes]
    imgs[-1].save(path, format="ICO", sizes=[(n, n) for n in sizes], append_images=imgs[:-1])


def main():
    render(256).save(ROOT / "flutter/assets/icon.png")
    render(256).save(ROOT / "dreamdesk/res/icon-256.png")
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    save_ico(ROOT / "flutter/assets/icon.ico", sizes)
    save_ico(ROOT / "flutter/windows/runner/resources/app_icon.ico", sizes)
    for p in ["flutter/assets/icon.png", "flutter/assets/icon.ico", "flutter/windows/runner/resources/app_icon.ico",
              "dreamdesk/res/icon-256.png"]:
        print(p, (ROOT / p).stat().st_size, "bytes")


if __name__ == "__main__":
    main()
