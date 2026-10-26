"""核对 ball_icons.rs 的换算（gen-ball-icons.py 的输出）：每个图标左边画原图、右边画换算后的路径（同样的线宽 / 填色），
各放大到 240 像素。对照页用无头浏览器截图，如
msedge --headless=new --screenshot=out.png --window-size=520,2900 --force-device-scale-factor=1 file:///.../check.html
python -I check_ball_icons.py html <rs> <图标目录> <输出 html>   生成对照页（再用无头 Edge 截图）
python -I check_ball_icons.py diff <截图>                      比较每对左右两半，打出不同像素占比"""
import re
import sys
import pathlib

ORDER_RE = re.compile(r'/// (\S+) (\S+)\npub const (\w+): Icon = Icon \{\n    grid: ([\d.]+),\n    fill: (\w+),\n    segs: &\[\n(.*?)\n    \],', re.S)
SIZE, GAP = 240, 20

if sys.argv[1] == 'html':
    rs, icons, out = pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]), pathlib.Path(sys.argv[4])
    rows = []
    for lib, name, const, grid, fill, body in ORDER_RE.findall(rs.read_text(encoding='utf-8')):
        d = []
        for seg in re.findall(r'(M|L|C|Z)(?:\(([^)]*)\))?', body):
            d.append(seg[0] + (' ' + seg[1].replace(',', ' ') if seg[1] else ''))
        src = (icons / f'{lib}-{name}.svg').read_text(encoding='utf-8')
        src = re.sub(r'<!--.*?-->', '', src, flags=re.S)
        src = re.sub(r'<svg\b', f'<svg width="{SIZE}" height="{SIZE}"', src, count=1)
        if fill == 'true':
            src = src.replace('<svg ', '<svg fill="#000" ', 1)
            conv = f'<svg width="{SIZE}" height="{SIZE}" viewBox="0 0 {grid} {grid}"><path fill="#000" d="{" ".join(d)}"/></svg>'
        else:
            src = re.sub(r'stroke-width="[\d.]+"', 'stroke-width="1.5"', src).replace('currentColor', '#000')
            conv = (f'<svg width="{SIZE}" height="{SIZE}" viewBox="0 0 {grid} {grid}" fill="none" stroke="#000" stroke-width="1.5" '
                    f'stroke-linecap="round" stroke-linejoin="round"><path d="{" ".join(d)}"/></svg>')
        rows.append(f'<div style="display:flex;gap:{GAP}px;margin-bottom:{GAP}px">{src}{conv}</div>')
    out.write_text('<!doctype html><body style="margin:0;background:#fff">' + ''.join(rows) + '</body>', encoding='utf-8')
    print(len(rows), 'rows')
else:
    from PIL import Image, ImageChops
    im = Image.open(sys.argv[2]).convert('L')
    y, n = 0, 0
    while y + SIZE <= im.height:
        a = im.crop((0, y, SIZE, y + SIZE))
        b = im.crop((SIZE + GAP, y, 2 * SIZE + GAP, y + SIZE))
        if a.getextrema() == (255, 255) and b.getextrema() == (255, 255):
            break
        diff = ImageChops.difference(a, b).point(lambda v: 255 if v > 64 else 0)
        ink = sum(1 for v in a.point(lambda v: 255 if v < 128 else 0).getdata() if v)
        bad = sum(1 for v in diff.getdata() if v)
        print(f'row {n}: ink {ink} px, differing {bad} px ({bad / max(ink, 1):.2%} of ink)')
        y += SIZE + GAP
        n += 1
