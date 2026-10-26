"""由 dreamdesk/res/icon.svg（用户 10-26 给的图标）生成各处要用的程序图标。

改了图标就在 Windows 上跑一次：python dreamdesk/scripts/gen-icons.py（要 Pillow；用系统自带的 Edge 无窗口把矢量图画成
1024 像素的透明底位图，再缩成各个尺寸），把生成的文件一起提交。生成：
- flutter/assets/icon.png、icon.ico：RustDesk 给定制客户端的入口，托盘、界面里的小图标用 png，Windows 快捷方式、
  「程序和功能」用 ico（src/tray.rs load_icon_from_asset、src/platform/windows.rs get_custom_icon）
- flutter/windows/runner/resources/app_icon.ico：程序文件内嵌的图标（任务栏、窗口、资源管理器）
- dreamdesk/res/icon-256.png：Linux 包装进 hicolor 主题的位图（矢量图直接用 icon.svg，见 dreamdesk/scripts/build-linux.sh）
"""
import subprocess
import tempfile
import time
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
SVG = ROOT / "dreamdesk/res/icon.svg"
EDGE = Path(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe")
BIG = 1024


def render_big(tmp: Path) -> Image.Image:
    page = tmp / "icon.html"
    page.write_text(
        '<html><body style="margin:0;background:transparent">'
        f'<img src="{SVG.as_uri()}" style="width:{BIG}px;height:{BIG}px;display:block"></body></html>',
        encoding="utf-8",
    )
    out = tmp / "big.png"
    subprocess.run(
        [str(EDGE), "--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1",
         "--default-background-color=00000000", f"--window-size={BIG},{BIG}", f"--screenshot={out}",
         f"--user-data-dir={tmp / 'profile'}", page.as_uri()],
        check=True, capture_output=True, timeout=120,
    )
    # msedge.exe 只是个启动器，交给后台进程画完就先退出了：等截图文件写完
    for _ in range(60):
        if out.exists() and out.stat().st_size > 0:
            time.sleep(0.5)
            break
        time.sleep(0.5)
    img = Image.open(out).convert("RGBA")
    if img.size != (BIG, BIG):
        raise SystemExit(f"Edge rendered {img.size}, expected {BIG}x{BIG}")
    return img


def main():
    with tempfile.TemporaryDirectory() as t:
        big = render_big(Path(t))
    sized = lambda n: big.resize((n, n), Image.LANCZOS)
    sized(256).save(ROOT / "flutter/assets/icon.png")
    sized(256).save(ROOT / "dreamdesk/res/icon-256.png")
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    imgs = [sized(n) for n in sizes]
    for p in ["flutter/assets/icon.ico", "flutter/windows/runner/resources/app_icon.ico"]:
        imgs[-1].save(ROOT / p, format="ICO", sizes=[(n, n) for n in sizes], append_images=imgs[:-1])
    for p in ["flutter/assets/icon.png", "flutter/assets/icon.ico", "flutter/windows/runner/resources/app_icon.ico",
              "dreamdesk/res/icon-256.png"]:
        print(p, (ROOT / p).stat().st_size, "bytes")


if __name__ == "__main__":
    main()
