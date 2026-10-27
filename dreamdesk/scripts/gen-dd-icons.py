"""由图标库原文件（dreamdesk/design/icons，各库原样下载，地址见 dreamdesk/design/图标清单.md）生成
flutter/lib/desktop/widgets/dd_icons.dart（路径照抄，不手改）。要加图标：把原文件放进 icons 目录、在下面的表里加一行，再跑一遍。
用法：python -I dreamdesk/scripts/gen-dd-icons.py"""
import pathlib
import re

repo = pathlib.Path(__file__).resolve().parents[2]
icons = repo / 'dreamdesk/design/icons'
out = repo / 'flutter/lib/desktop/widgets/dd_icons.dart'

# 枚举名 → (库, 原文件名)；顺序就是枚举顺序。前 21 个是原来就有的名字（代码里在用），后面是新加的
STROKE = [  # Lucide
    ('user', 'user'), ('info', 'info'), ('arrowUpCircle', 'circle-arrow-up'), ('settings', 'settings'),
    ('devices', 'monitor-smartphone'), ('chevronUp', 'chevron-up'), ('chevronDown', 'chevron-down'),
    ('folderUp', 'folder-up'), ('terminal', 'square-terminal'), ('arrowLeftRight', 'arrow-left-right'),
    ('moreVertical', 'ellipsis-vertical'), ('arrowRight', 'arrow-right'), ('close', 'x'), ('eye', 'eye'),
    ('eyeOff', 'eye-off'), ('monitor', 'monitor'), ('mic', 'mic'), ('micOff', 'mic-off'), ('signal', 'signal'),
    ('arrowLeft', 'arrow-left'), ('lock', 'lock'), ('mouse', 'mouse'), ('wrench', 'wrench'), ('more', 'ellipsis'),
    ('panelTop', 'panel-top'), ('volume', 'volume-1'), ('power', 'power'), ('maximize', 'maximize'),
    ('minimize', 'minimize'), ('logOut', 'log-out'), ('layoutGrid', 'layout-grid'), ('check', 'check'),
    ('ban', 'ban'), ('record', 'circle-dot'), ('camera', 'camera'), ('keyboard', 'keyboard'),
    ('refresh', 'rotate-ccw'), ('zap', 'zap'), ('wifiOff', 'wifi-off'), ('circleAlert', 'circle-alert'),
    ('circleX', 'circle-x'), ('circleCheck', 'circle-check'), ('shieldAlert', 'shield-alert'),
    ('hourglass', 'hourglass'), ('shieldCheck', 'shield-check'), ('shield', 'shield'),
    ('chevronLeft', 'chevron-left'), ('chevronRight', 'chevron-right'), ('house', 'house'),
    ('folderPlus', 'folder-plus'), ('trash', 'trash-2'), ('search', 'search'), ('folder', 'folder'),
    ('file', 'file'), ('hardDrive', 'hard-drive'), ('plus', 'plus'), ('triangleAlert', 'triangle-alert'),
    ('messageSquare', 'message-square'),
    # 照 UU 的信号格（三根竖条）
    ('signalBars', 'chart-no-axes-column-increasing'),
    # 窗口按钮（10-27 用户选：和标题栏上别的图标同一家，不用 Fluent）
    ('winMinimize', 'minus'), ('winMaximize', 'square'), ('winClose', 'x'),
    # 设置页左侧分类（10-27 用户：Material 换成 Lucide；安全、显示、账户、关于用已有的 shield / monitor / user / info）
    ('sliders', 'sliders-horizontal'), ('network', 'network'), ('printer', 'printer'),
]
FILL = [  # (枚举名, 前缀, 原文件名)
    ('menu', 'fluent', 'navigation_20_regular'),
    ('controlCenter', 'fluent', 'toggle_multiple_20_regular'),
    # 窗口「还原」（10-27 用户：Fluent 的两个叠着的方框最合适）
    ('winRestore', 'fluent', 'square_multiple_16_regular'),
    # 屏幕标签（10-27 用户：照 UU 的长方形 + 底部粗条）
    ('screen', 'material', 'video_label'),
    # 「全部显示屏」标签（一个画面里两块屏）、「铺满我所有显示器」按钮（两台显示器），10-27 原型 RemoteMulti.html
    ('allScreens', 'material', 'splitscreen_landscape'),
    ('myDisplays', 'material', 'tv_displays'),
    ('osWindows', 'fa', 'windows'),
    ('osLinux', 'fa', 'linux'),
    ('osMac', 'fa', 'apple'),
]


def read(kind, name):
    text = (icons / f'{kind}-{name}.svg').read_text(encoding='utf-8')
    text = re.sub(r'<!--.*?-->', '', text, flags=re.S)
    m = re.search(r'<svg([^>]*)>(.*)</svg>', text, re.S)
    vb = re.search(r'viewBox="([^"]+)"', m.group(1)).group(1)
    inner = re.sub(r'\s*\n\s*', '', m.group(2)).strip()
    inner = re.sub(r'\s*/>', '/>', inner)
    assert "'" not in inner and '$' not in inner
    return vb, inner


names = [n for n, _ in STROKE] + [n for n, _, _ in FILL]
order = ['menu', 'user', 'info', 'arrowUpCircle', 'settings', 'devices', 'chevronUp', 'chevronDown', 'folderUp',
         'terminal', 'arrowLeftRight', 'moreVertical', 'arrowRight', 'close', 'eye', 'eyeOff', 'monitor', 'mic',
         'micOff', 'controlCenter', 'signal']
order += [n for n in names if n not in order]

lines = []
lines.append('// DreamDesk 受管版界面的图标，全部是开源图标库的原图（路径照抄、不手改），清单见 dreamdesk/design/图标清单.md。')
lines.append('// 本文件由 dreamdesk/scripts/gen-dd-icons.py 从图标库原文件生成（各库的许可声明原文在文件末尾）：')
lines.append('// - Lucide（lucide-static 0.460.0，ISC 许可）：描边图标，线宽统一 1.5（界面规范；Lucide 支持调线宽，路径不动）')
lines.append('// - Fluent UI System Icons（@fluentui/svg-icons 1.1.292，MIT 许可）：填色图标，按显示尺寸用对应号的原图，不缩放')
lines.append('// - Font Awesome Free 6.6.0 品牌图标（CC BY 4.0 许可，https://fontawesome.com/license/free）：系统标志')
lines.append('// - Material Symbols（@material-symbols/svg-400 0.23.0 outlined，Apache 2.0 许可）：屏幕标签（video_label，照 UU 的长方形 + 底部粗条）')
lines.append("import 'package:flutter/material.dart';")
lines.append("import 'package:flutter_svg/flutter_svg.dart';")
lines.append('')
lines.append('enum DdIcon {')
for n in order:
    lines.append(f'  {n},')
lines.append('}')
lines.append('')
lines.append("const _strokeHead =")
lines.append("    '<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"#000\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\">';")
lines.append('')
lines.append('/// Lucide 描边图标（24 格）')
lines.append('const Map<DdIcon, String> _stroke = {')
for n, f in STROKE:
    vb, inner = read('lucide', f)
    assert vb == '0 0 24 24', f
    lines.append(f"  // lucide: {f}")
    lines.append(f"  DdIcon.{n}: '{inner}',")
lines.append('};')
lines.append('')
lines.append('/// 填色图标：(viewBox, 内容)。Fluent 的 20 号按 20 像素用')
lines.append('const Map<DdIcon, (String, String)> _fill = {')
for n, kind, f in FILL:
    vb, inner = read(kind, f)
    lines.append(f"  // {'fluent' if kind == 'fluent' else 'font awesome'}: {f}")
    lines.append(f"  DdIcon.{n}: ('{vb}', '{inner}'),")
lines.append('};')
lines.append('')
lines.append("/// 拼好的 SVG 串：同一个图标 + 线宽只拼一次（flutter_svg 拿整串当缓存键，每次拼新串、算哈希是白费功夫）")
lines.append("final _svgs = <(DdIcon, double), String>{};")
lines.append("")
lines.append("String _svgOf(DdIcon icon, double strokeWidth) => _svgs.putIfAbsent((icon, strokeWidth), () {")
lines.append("  final fill = _fill[icon];")
lines.append("  if (fill != null) return '<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"${fill.$1}\" fill=\"#000\">${fill.$2}</svg>';")
lines.append("  final head = strokeWidth == 1.5 ? _strokeHead : _strokeHead.replaceFirst('stroke-width=\"1.5\"', 'stroke-width=\"$strokeWidth\"');")
lines.append("  return '$head${_stroke[icon]}</svg>';")
lines.append("});")
lines.append("")
lines.append("/// [strokeWidth]：描边图标的线宽（Lucide 的 24 格里算），默认 1.5；填色图标不管它")
lines.append("Widget ddIcon(DdIcon icon, {double size = 18, Color color = const Color(0xFF1F2329), double strokeWidth = 1.5}) =>")
lines.append("    SvgPicture.string(_svgOf(icon, strokeWidth), width: size, height: size, colorFilter: ColorFilter.mode(color, BlendMode.srcIn));")

# 「全部设备」：Phosphor circles-four（fill）。原路径是 4 个圆连在一条里，照 UU 拆开分别上两种蓝（对角同色）；
# 第二个圆开头是相对的 m56,40（上一个圆闭合后的当前点是 120,80），换成等价的 M176,120，其余照抄
src = (icons / 'phosphor-circles-four-fill.svg').read_text(encoding='utf-8')
d = re.search(r'<path d="([^"]+)"', src).group(1)
parts = [p + 'Z' for p in d.split('Z') if p]
assert len(parts) == 4 and parts[1].startswith('m56,40') and parts[3].startswith('m96,0'), parts
rebuilt = parts[0] + parts[1] + parts[2] + parts[3]
assert rebuilt == d, 'split must round-trip'
# 相对的 moveto 以上一个子路径的起点为基准（闭合后当前点回到起点）：第二段起点 120,80 + (56,40)；第四段起点 80,136 + (96,0)
parts[1] = 'M176,120' + parts[1][len('m56,40'):]
parts[3] = 'M176,136' + parts[3][len('m96,0'):]
assert all(p.startswith('M') for p in parts), parts
dark, light = '#2f6fea', '#86aef3'
paths = ''.join(f'<path fill="{c}" d="{p}"/>' for c, p in zip([dark, light, light, dark], parts))
lines.append('')
lines.append('/// 「全部设备」的图标（照 UU：2×2 四个实心圆，对角同色、两种蓝）。Phosphor Icons 的 circles-four（fill，@phosphor-icons/core')
lines.append('/// 2.1.1，MIT 许可），原路径按 4 个圆拆开分别上色，形状不变；viewBox 裁到四个圆的范围，和系统小方块一样占满尺寸')
lines.append(f"const _allDevices = '<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"40 40 176 176\">{paths}</svg>';")
lines.append('')
lines.append('Widget ddAllDevicesIcon({double size = 20}) => SvgPicture.string(_allDevices, width: size, height: size);')

# 许可声明：ISC / MIT 要求副本里保留版权和许可声明，原文照抄（各库发布的 LICENSE 文件）；Font Awesome 图标是 CC BY 4.0，注明出处
lines.append('')
lines.append('// ---- 图标库许可声明（原文） ----')
for title, f in [('Lucide', 'lucide'), ('Fluent UI System Icons', 'fluent'), ('Phosphor Icons', 'phosphor'),
                 ('Material Symbols', 'material-symbols')]:
    text = (icons / f'LICENSE-{f}.txt').read_text(encoding='utf-8').strip()
    lines.append('//')
    lines.append(f'// {title}:')
    for t in text.splitlines():
        lines.append(f'// {t}'.rstrip())
lines.append('//')
lines.append('// Font Awesome Free 6.6.0 brand icons (windows, linux, apple) by Fonticons, Inc., https://fontawesome.com,')
lines.append('// licensed under CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/). Paths unchanged.')
out.write_text('\n'.join(lines) + '\n', encoding='utf-8', newline='\n')
print(f'{len(order)} icons -> {out}')
