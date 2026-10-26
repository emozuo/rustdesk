"""由图标库原文件生成 src/ddpro/ball_icons.rs（专业窗口悬浮球的图标，tiny-skia 画）。
SVG 的路径、圆、矩形、直线、折线照原样换算成绝对坐标的 移到 / 直线 / 三次曲线 / 闭合 四种指令：
圆弧按 SVG 规范的端点参数换算成中心参数，再每段不超过 90 度用三次曲线逼近（标准做法，误差远小于 1 像素），形状不改。
原文件在 dreamdesk/design/icons；换算对不对用 check-ball-icons.py 和原图逐像素比。
用法：python -I dreamdesk/scripts/gen-ball-icons.py"""
import math
import pathlib
import re

repo = pathlib.Path(__file__).resolve().parents[2]
icons = repo / 'dreamdesk/design/icons'
out = repo / 'src/ddpro/ball_icons.rs'

# 常量名 → (文件名, 是否填色)
ICONS = [
    ('BALL', 'fluent-toggle_multiple_20_regular', True),
    ('MONITOR', 'lucide-monitor', False),
    ('RESOLUTION', 'lucide-maximize-2', False),
    ('SIGNAL', 'lucide-signal', False),
    ('MIC', 'lucide-mic', False),
    ('HDR', 'lucide-sun', False),
    ('KEYBOARD', 'lucide-keyboard', False),
    ('LOCK', 'lucide-lock', False),
    ('LOG_OUT', 'lucide-log-out', False),
    ('CHEVRON_RIGHT', 'lucide-chevron-right', False),
    ('CHECK', 'lucide-check', False),
]

NUM = re.compile(r'[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?')


def arc_to_cubics(x1, y1, rx, ry, phi_deg, fa, fs, x2, y2):
    if (x1, y1) == (x2, y2):
        return []
    rx, ry = abs(rx), abs(ry)
    if rx == 0 or ry == 0:
        return [('L', x2, y2)]
    phi = math.radians(phi_deg)
    c, s = math.cos(phi), math.sin(phi)
    dx, dy = (x1 - x2) / 2, (y1 - y2) / 2
    x1p, y1p = c * dx + s * dy, -s * dx + c * dy
    lam = x1p ** 2 / rx ** 2 + y1p ** 2 / ry ** 2
    if lam > 1:
        rx, ry = rx * math.sqrt(lam), ry * math.sqrt(lam)
    num = rx ** 2 * ry ** 2 - rx ** 2 * y1p ** 2 - ry ** 2 * x1p ** 2
    den = rx ** 2 * y1p ** 2 + ry ** 2 * x1p ** 2
    coef = math.sqrt(max(0.0, num / den)) * (-1 if fa == fs else 1)
    cxp, cyp = coef * rx * y1p / ry, -coef * ry * x1p / rx
    cx = c * cxp - s * cyp + (x1 + x2) / 2
    cy = s * cxp + c * cyp + (y1 + y2) / 2

    def ang(ux, uy, vx, vy):
        a = math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)
        return a

    t1 = ang(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
    dt = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
    if not fs and dt > 0:
        dt -= 2 * math.pi
    elif fs and dt < 0:
        dt += 2 * math.pi
    n = max(1, math.ceil(abs(dt) / (math.pi / 2) - 1e-9))
    d = dt / n
    k = 4 / 3 * math.tan(d / 4)

    def pt(ex, ey):
        return (c * rx * ex - s * ry * ey + cx, s * rx * ex + c * ry * ey + cy)

    segs = []
    a = t1
    for _ in range(n):
        b = a + d
        p1 = pt(math.cos(a) - k * math.sin(a), math.sin(a) + k * math.cos(a))
        p2 = pt(math.cos(b) + k * math.sin(b), math.sin(b) - k * math.cos(b))
        p = pt(math.cos(b), math.sin(b))
        segs.append(('C', *p1, *p2, *p))
        a = b
    # 最后一个点精确落在终点（消掉浮点误差）
    segs[-1] = segs[-1][:5] + (x2, y2)
    return segs


def parse_path(d):
    """返回绝对坐标的 M / L / C / Z 列表"""
    toks = re.findall(r'[MmLlHhVvCcSsQqTtAaZz]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?', d)
    i, out = 0, []
    cx = cy = sx = sy = 0.0
    last_c = last_q = None
    cmd = None

    def num():
        nonlocal i
        v = float(toks[i])
        i += 1
        return v

    def flag():
        # 圆弧的两个标志可能挤在一起（如 "01"）：逐字符取
        nonlocal i
        t = toks[i]
        if len(t) > 1 and t[0] in '01' and not t.startswith(('0.', '1.')):
            toks[i] = t[1:]
            return int(t[0])
        i += 1
        return int(float(t))

    while i < len(toks):
        t = toks[i]
        if re.fullmatch(r'[A-Za-z]', t):
            cmd = t
            i += 1
            if cmd in 'Zz':
                out.append(('Z',))
                cx, cy = sx, sy
                last_c = last_q = None
                continue
        rel = cmd.islower()
        C = cmd.upper()
        ox, oy = (cx, cy) if rel else (0.0, 0.0)
        if C == 'M':
            x, y = num() + ox, num() + oy
            out.append(('M', x, y))
            cx, cy, sx, sy = x, y, x, y
            cmd = 'l' if rel else 'L'  # 后续的坐标对按直线
            last_c = last_q = None
        elif C == 'L':
            x, y = num() + ox, num() + oy
            out.append(('L', x, y))
            cx, cy = x, y
            last_c = last_q = None
        elif C == 'H':
            x = num() + (cx if rel else 0.0)
            out.append(('L', x, cy))
            cx = x
            last_c = last_q = None
        elif C == 'V':
            y = num() + (cy if rel else 0.0)
            out.append(('L', cx, y))
            cy = y
            last_c = last_q = None
        elif C == 'C':
            x1, y1, x2, y2, x, y = num() + ox, num() + oy, num() + ox, num() + oy, num() + ox, num() + oy
            out.append(('C', x1, y1, x2, y2, x, y))
            cx, cy, last_c, last_q = x, y, (x2, y2), None
        elif C == 'S':
            x1, y1 = (2 * cx - last_c[0], 2 * cy - last_c[1]) if last_c else (cx, cy)
            x2, y2, x, y = num() + ox, num() + oy, num() + ox, num() + oy
            out.append(('C', x1, y1, x2, y2, x, y))
            cx, cy, last_c, last_q = x, y, (x2, y2), None
        elif C == 'Q':
            qx, qy, x, y = num() + ox, num() + oy, num() + ox, num() + oy
            out.append(('C', cx + 2 / 3 * (qx - cx), cy + 2 / 3 * (qy - cy), x + 2 / 3 * (qx - x), y + 2 / 3 * (qy - y), x, y))
            cx, cy, last_q, last_c = x, y, (qx, qy), None
        elif C == 'T':
            qx, qy = (2 * cx - last_q[0], 2 * cy - last_q[1]) if last_q else (cx, cy)
            x, y = num() + ox, num() + oy
            out.append(('C', cx + 2 / 3 * (qx - cx), cy + 2 / 3 * (qy - cy), x + 2 / 3 * (qx - x), y + 2 / 3 * (qy - y), x, y))
            cx, cy, last_q, last_c = x, y, (qx, qy), None
        elif C == 'A':
            rx, ry, rot = num(), num(), num()
            fa, fs = flag(), flag()
            x, y = num() + ox, num() + oy
            out.extend(arc_to_cubics(cx, cy, rx, ry, rot, fa, fs, x, y))
            cx, cy = x, y
            last_c = last_q = None
        else:
            raise ValueError(f'unsupported command {cmd}')
    return out


def ellipse(cx, cy, rx, ry):
    d = f'M{cx + rx} {cy}A{rx} {ry} 0 0 1 {cx} {cy + ry}A{rx} {ry} 0 0 1 {cx - rx} {cy}' \
        f'A{rx} {ry} 0 0 1 {cx} {cy - ry}A{rx} {ry} 0 0 1 {cx + rx} {cy}Z'
    return parse_path(d)


def rect(a):
    x, y, w, h = (float(a.get(k, 0)) for k in ('x', 'y', 'width', 'height'))
    rx = float(a.get('rx', a.get('ry', 0)))
    ry = float(a.get('ry', rx))
    if rx == 0:
        return parse_path(f'M{x} {y}H{x + w}V{y + h}H{x}Z')
    return parse_path(f'M{x + rx} {y}H{x + w - rx}A{rx} {ry} 0 0 1 {x + w} {y + ry}V{y + h - ry}'
                      f'A{rx} {ry} 0 0 1 {x + w - rx} {y + h}H{x + rx}A{rx} {ry} 0 0 1 {x} {y + h - ry}'
                      f'V{y + ry}A{rx} {ry} 0 0 1 {x + rx} {y}Z')


def attrs(tag):
    return dict(re.findall(r'([\w-]+)="([^"]*)"', tag))


def convert(name):
    text = (icons / f'{name}.svg').read_text(encoding='utf-8')
    text = re.sub(r'<!--.*?-->', '', text, flags=re.S)
    vb = [float(v) for v in re.search(r'viewBox="([^"]+)"', text).group(1).split()]
    segs = []
    for m in re.finditer(r'<(path|circle|ellipse|rect|line|polyline|polygon)\b([^>]*)>', text):
        kind, a = m.group(1), attrs(m.group(2))
        if kind == 'path':
            segs += parse_path(a['d'])
        elif kind == 'circle':
            r = float(a['r'])
            segs += ellipse(float(a['cx']), float(a['cy']), r, r)
        elif kind == 'ellipse':
            segs += ellipse(float(a['cx']), float(a['cy']), float(a['rx']), float(a['ry']))
        elif kind == 'rect':
            segs += rect(a)
        elif kind == 'line':
            segs += [('M', float(a['x1']), float(a['y1'])), ('L', float(a['x2']), float(a['y2']))]
        else:
            pts = [float(v) for v in NUM.findall(a['points'])]
            segs.append(('M', pts[0], pts[1]))
            segs += [('L', pts[j], pts[j + 1]) for j in range(2, len(pts), 2)]
            if kind == 'polygon':
                segs.append(('Z',))
    assert vb[0] == 0 and vb[1] == 0 and vb[2] == vb[3], name
    return vb[2], segs


def fmt(v):
    s = f'{round(v, 4):.4f}'.rstrip('0').rstrip('.')
    if s in ('-0', ''):
        s = '0'
    return s + ('' if '.' in s else '.0')


lines = [
    '//! 专业窗口悬浮球的图标：全部是开源图标库的原图（清单见 dreamdesk/design/图标清单.md），本文件由 dreamdesk/scripts/gen-ball-icons.py 从原文件生成，不手改。',
    '//! SVG 的路径、圆、矩形、直线、折线换算成绝对坐标的 移到 / 直线 / 三次曲线 / 闭合（圆弧按 SVG 规范换成三次曲线逼近，',
    '//! 形状不改），由 `ball.rs` 用 tiny-skia 画。',
    '//! - Lucide（lucide-static 0.460.0，ISC 许可）：描边图标，24 格，线宽按界面规范 1.5',
    '//! - Fluent UI System Icons（@fluentui/svg-icons 1.1.292，MIT 许可）：填色图标，20 号原图',
    '//! 两个库的许可声明原文在文件末尾。',
    '',
    '/// 一段路径指令（坐标在图标自己的格子里）',
    'pub enum Seg {',
    '    M(f32, f32),',
    '    L(f32, f32),',
    '    C(f32, f32, f32, f32, f32, f32),',
    '    Z,',
    '}',
    '',
    '/// 图标：格子边长（Lucide 24、Fluent 20 号是 20）、填色还是描边、路径',
    'pub struct Icon {',
    '    pub grid: f32,',
    '    pub fill: bool,',
    '    pub segs: &\'static [Seg],',
    '}',
    '',
    'use Seg::{C, L, M, Z};',
]
for const, f, fill in ICONS:
    grid, segs = convert(f)
    lines.append('')
    lines.append(f'/// {f.replace("-", " ", 1)}')
    lines.append(f'pub const {const}: Icon = Icon {{')
    lines.append(f'    grid: {fmt(grid)},')
    lines.append(f'    fill: {"true" if fill else "false"},')
    lines.append('    segs: &[')
    for s in segs:
        if s[0] == 'Z':
            lines.append('        Z,')
        else:
            lines.append(f'        {s[0]}({", ".join(fmt(v) for v in s[1:])}),')
    lines.append('    ],')
    lines.append('};')

lines.append('')
lines.append('// ---- 图标库许可声明（原文） ----')
for title, f in [('Lucide', 'lucide'), ('Fluent UI System Icons', 'fluent')]:
    text = (icons / f'LICENSE-{f}.txt').read_text(encoding='utf-8').strip()
    lines.append('//')
    lines.append(f'// {title}:')
    for t in text.splitlines():
        lines.append(f'// {t}'.rstrip())
out.write_text('\n'.join(lines) + '\n', encoding='utf-8', newline='\n')
print(f'{len(ICONS)} icons -> {out}')
