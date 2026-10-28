// 首页的左栏行、状态标记，和右边的设备卡片 / 详情 / 插画。从 dd_home_page.dart 拆出来的，私有类照常互相可见
part of 'dd_home_page.dart';

/// 左栏的一行（照 UU）：底色块从左栏边上 4 像素开始；选中时浅灰圆角底，蓝竖线在底色块里面、贴最左边（4×16、两头圆、上下居中）
class _SideRow extends StatelessWidget {
  final Widget icon;
  final String label;
  /// 紧跟在名字后面的（「本机」标签）；[trailing] 靠右
  final Widget? labelSuffix;
  final Widget? trailing;
  final Color? dot;
  final bool selected;
  final bool indent;
  final bool dim;
  final VoidCallback onTap;
  final VoidCallback? onDoubleTap;
  /// 认双击用的：两台工作站可能同名，按设备号认，不然先点一台、马上点另一台会被当成双击
  final String? clickId;
  const _SideRow(
      {required this.icon,
      required this.label,
      this.labelSuffix,
      this.trailing,
      this.dot,
      this.selected = false,
      this.indent = false,
      this.dim = false,
      required this.onTap,
      this.onDoubleTap,
      this.clickId});

  @override
  Widget build(BuildContext context) {
    // 底色撑满这一行（上下各留 2 像素）；竖线画在底色上面，所以放在 Stack 的后面
    return SizedBox(
      height: DdSize.rowHeight,
      child: Stack(fit: StackFit.expand, children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(4, 2, 4, 2),
          child: Material(
            color: selected ? DdColors.selected : Colors.transparent,
            borderRadius: BorderRadius.circular(DdRadius.md),
            child: InkWell(
              borderRadius: BorderRadius.circular(DdRadius.md),
              hoverColor: DdColors.hover,
              onTap: () {
                onTap();
                // 每一行都记一次点击（没有双击动作的行也记）：不然依次点 A、「全部设备」、A 会被当成双击 A
                if (_Clicks.isDouble('side:${clickId ?? label}')) onDoubleTap?.call();
              },
              child: Padding(
                padding: EdgeInsets.only(left: indent ? 24 : 14, right: 14),
                child: Row(children: [
                  if (indent)
                    SizedBox(
                      width: 18,
                      child: dot == null
                          ? null
                          : Align(
                              alignment: Alignment.centerLeft,
                              child: DdDot(color: dot!),
                            ),
                    ),
                  icon,
                  const SizedBox(width: 12),
                  Expanded(
                    child: Row(children: [
                      Flexible(
                        child: Text(label,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 14, color: dim ? DdColors.ink3 : DdColors.ink)),
                      ),
                      if (labelSuffix != null) labelSuffix!,
                    ]),
                  ),
                  if (trailing != null) trailing!,
                ]),
              ),
            ),
          ),
        ),
        if (selected)
          Positioned(
            left: 4,
            top: 12,
            bottom: 12,
            child: IgnorePointer(
              child: Container(width: 4, decoration: BoxDecoration(color: DdColors.accent, borderRadius: BorderRadius.circular(2))),
            ),
          ),
      ]),
    );
  }
}

/// 系统图标：彩色小方块里白色的系统标识（Windows 蓝、Linux 橙、Mac 黑），离线时灰
class _OsBadge extends StatelessWidget {
  final String os;
  final bool online;
  final double size;
  const _OsBadge({required this.os, required this.online, required this.size});

  @override
  Widget build(BuildContext context) {
    // 系统标志用 Font Awesome 的品牌图标（dd_icons.dart）
    final (DdIcon icon, Color color) = switch (ddOsKind(os)) {
      DdOsKind.linux => (DdIcon.osLinux, DdColors.osLinux),
      DdOsKind.mac => (DdIcon.osMac, DdColors.osMac),
      DdOsKind.windows => (DdIcon.osWindows, DdColors.osWindows),
    };
    return Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: online ? color : DdColors.offline,
        borderRadius: BorderRadius.circular(size * 0.22),
      ),
      child: ddIcon(icon, size: size * 0.6, color: DdColors.onAccent),
    );
  }
}

class _StatePill extends StatelessWidget {
  final String text;
  final Color color;
  const _StatePill({required this.text, required this.color});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(8, 3, 10, 3),
      decoration: BoxDecoration(color: DdColors.pill, borderRadius: BorderRadius.circular(DdRadius.pill)),
      child: Row(mainAxisSize: MainAxisSize.min, children: [
        DdDot(color: color, size: 7),
        const SizedBox(width: 5),
        Text(text, style: const TextStyle(color: DdColors.onAccent, fontSize: 13)),
      ]),
    );
  }
}

/// 内容区居中的一个状态（照原型 HomeStates）：图标、一句话、（灰色说明）、（按钮），间距 12。[dim]：那句话也用灰色（加载中）
class _StateView extends StatelessWidget {
  final Widget icon;
  final String title;
  final String? subtitle;
  final Widget? action;
  final bool dim;
  const _StateView({required this.icon, required this.title, this.subtitle, this.action, this.dim = false});

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Column(mainAxisSize: MainAxisSize.min, children: [
        icon,
        const SizedBox(height: 12),
        Text(title, textAlign: TextAlign.center, style: TextStyle(fontSize: 14, color: dim ? DdColors.ink2 : DdColors.ink)),
        if (subtitle != null) ...[
          const SizedBox(height: 12),
          Text(subtitle!, textAlign: TextAlign.center, style: const TextStyle(fontSize: 13, color: DdColors.ink2)),
        ],
        if (action != null) ...[const SizedBox(height: 12), action!],
      ]),
    );
  }
}

/// 内容区右下角的「连不上服务器」提示条（原型 HomeHints 的 dd-netwarn）：暖色底、橙色断网图标、一句话 + 文字按钮「再试」
class _NetWarn extends StatelessWidget {
  final String text;
  final VoidCallback onRetry;
  const _NetWarn({required this.text, required this.onRetry});

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 32,
      padding: const EdgeInsets.only(left: 12, right: 6),
      decoration: BoxDecoration(color: DdColors.warnBg, borderRadius: BorderRadius.circular(DdRadius.md), boxShadow: DdShadow.card),
      child: Row(mainAxisSize: MainAxisSize.min, children: [
        ddIcon(DdIcon.wifiOff, size: 16, color: DdColors.busy),
        const SizedBox(width: 8),
        Text(text, style: const TextStyle(fontSize: 13, color: DdColors.ink)),
        const SizedBox(width: 8),
        TextButton(
          onPressed: onRetry,
          style: TextButton.styleFrom(
            minimumSize: const Size(0, 24),
            padding: const EdgeInsets.symmetric(horizontal: 6),
            tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            textStyle: const TextStyle(fontSize: 13),
          ),
          child: Text(translate('Retry')),
        ),
      ]),
    );
  }
}

/// 详情大卡片悬停动效的时长（原型 .dd-hero 的 .18s）
const _kHeroAnim = Duration(milliseconds: 180);

/// 详情大卡片的上半：远程桌面的「壁纸」（没有远程截图，按系统用它的默认壁纸）+ 偏下的「进入桌面 →」
class _EnterDesktop extends StatefulWidget {
  final String label;
  final bool arrow;
  final VoidCallback? onTap;
  final String os;
  final bool online;
  const _EnterDesktop({required this.label, required this.arrow, required this.os, required this.online, this.onTap});

  @override
  State<_EnterDesktop> createState() => _EnterDesktopState();
}

class _EnterDesktopState extends State<_EnterDesktop> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: widget.onTap == null ? SystemMouseCursors.basic : SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        // 悬停动效只有一个进度 t（0 → 1），背景和文字一起用
        child: TweenAnimationBuilder<double>(
          tween: Tween(end: _hover && widget.onTap != null ? 1 : 0),
          duration: _kHeroAnim,
          curve: Curves.ease,
          // 壁纸不随 t 变，建一次传进去
          child: _OsWallpaper(os: widget.os, online: widget.online),
          builder: (context, t, wallpaper) => Stack(fit: StackFit.expand, children: [
            // 照 UU（原型 HomeDetail .dd-hero）：能连时鼠标停上去背景放大一点、变虚。
            // 模糊用 enabled 开关、不按 t 增删这一层：增删会改部件树，每次悬停开始、结束壁纸图都要重新挂一次
            ClipRect(
              child: Transform.scale(
                scale: 1 + 0.06 * t,
                child: ImageFiltered(
                  enabled: t > 0,
                  imageFilter: ui.ImageFilter.blur(sigmaX: 6 * t, sigmaY: 6 * t),
                  child: wallpaper,
                ),
              ),
            ),
            // 能连时照 UU 只写白字加箭头，不做成按钮，放在偏下（约八成高处）；鼠标停上去往上抬 8、字和箭头的间距 10 拉到 20，
            // 字大小不变（原型 .dd-hero__go，10-27 用户）。字带一点阴影，浅色壁纸上也看得清。连不了时只在正中写状态
            if (widget.arrow)
              Align(
                alignment: const Alignment(0, 0.58),
                child: Transform.translate(
                  offset: Offset(0, -8 * t),
                  child: Row(mainAxisSize: MainAxisSize.min, children: [
                    Text(widget.label,
                        style: const TextStyle(
                            color: DdColors.onAccent,
                            fontSize: 16,
                            shadows: [Shadow(color: Color(0x66000000), blurRadius: 6, offset: Offset(0, 1))])),
                    SizedBox(width: 10 + 10 * t),
                    ddIcon(DdIcon.arrowRight, size: 18, color: DdColors.onAccent),
                  ]),
                ),
              )
            else
              Center(
                // 离线时底是浅灰，白字看不清，用深色字
                child: Text(widget.label,
                    style: TextStyle(color: widget.online ? DdColors.onAccent : DdColors.ink2, fontSize: 16, letterSpacing: 1)),
              ),
          ]),
        ),
      ),
    );
  }
}

/// 设备卡片、详情大卡片的背景（原型 dd.css 的 dd-wall--*，10-27 用户：先按最简单的预置）：按被控端系统用它的默认壁纸——
/// Windows 是 Windows 11 默认壁纸 Bloom（版权归微软，用户知情先这样用），Linux 是 Ubuntu 默认壁纸 warty-final-ubuntu
/// （ubuntu-wallpapers，CC-BY-SA-3.0，出处见 dreamdesk/design/图标清单.md）；macOS 没有公开原图，和离线一样用画的
class _OsWallpaper extends StatelessWidget {
  final String os;
  final bool online;
  const _OsWallpaper({required this.os, required this.online});

  @override
  Widget build(BuildContext context) {
    final asset = !online
        ? null
        : switch (ddOsKind(os)) {
            DdOsKind.linux => 'assets/dd/wallpaper_linux.jpg',
            DdOsKind.mac => null,
            DdOsKind.windows => 'assets/dd/wallpaper_windows.jpg',
          };
    if (asset == null) return CustomPaint(painter: _DrawnWallpaper(online: online), size: Size.infinite);
    return Image.asset(asset, fit: BoxFit.cover, width: double.infinity, height: double.infinity);
  }
}

/// 没有壁纸图时画的背景（只给 [_OsWallpaper] 用）：离线一律灰；在线只剩 macOS 走到这里，画它的近似配色
/// （数值照原型 dd.css 的 dd-wall--mac，CSS 的线性渐变角度、椭圆光斑的位置和半径都按画布的百分比）
class _DrawnWallpaper extends CustomPainter {
  final bool online;
  const _DrawnWallpaper({required this.online});

  /// CSS 的 linear-gradient 角度：0 度朝上、顺时针
  static void _linear(Canvas canvas, Rect rect, double deg, List<Color> colors, List<double> stops) {
    final r = deg * math.pi / 180;
    final dx = math.sin(r), dy = -math.cos(r);
    canvas.drawRect(
      rect,
      Paint()
        ..shader = LinearGradient(begin: Alignment(-dx, -dy), end: Alignment(dx, dy), colors: colors, stops: stops)
            .createShader(rect),
    );
  }

  /// CSS 的 radial-gradient(rx% ry% at cx% cy%, color 0, transparent stop)：椭圆光斑，先画单位圆再按两个半径拉伸
  static void _glow(Canvas canvas, Size s, double cx, double cy, double rx, double ry, Color color, double stop) {
    canvas.save();
    canvas.translate(s.width * cx, s.height * cy);
    canvas.scale(s.width * rx, s.height * ry);
    const unit = Rect.fromLTWH(-1, -1, 2, 2);
    canvas.drawCircle(
      Offset.zero,
      1,
      Paint()..shader = RadialGradient(colors: [color, color.withOpacity(0)], stops: [0, stop]).createShader(unit),
    );
    canvas.restore();
  }

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    // 光斑比画布大，不裁会画到下面的白色名字栏、操作栏上
    canvas.clipRect(rect);
    if (!online) {
      _linear(canvas, rect, 135, const [Color(0xFFC9CDD4), Color(0xFFB3B8C0)], const [0, 1]);
      return;
    }
    _linear(canvas, rect, 160, const [Color(0xFF24305E), Color(0xFF1A1F3D), Color(0xFF3B2450)], const [0, 0.55, 1]);
    _glow(canvas, size, 0.30, 0.75, 0.60, 0.80, const Color(0xFF7A6CFF), 0.60);
  }

  @override
  bool shouldRepaint(covariant _DrawnWallpaper oldDelegate) => oldDelegate.online != online;
}

class _ActionCell extends StatelessWidget {
  final DdIcon icon;
  final String label;
  final VoidCallback? onTap;
  const _ActionCell({required this.icon, required this.label, this.onTap});

  @override
  Widget build(BuildContext context) {
    // 用不了（离线、别人在用、本机）是 UU 的灰，也没有悬停底色（InkWell 没有 onTap 时不画）
    final color = onTap == null ? DdColors.unavailable : DdColors.ink;
    return InkWell(
      onTap: onTap,
      // 撑满这一格（操作栏整行高）：悬停底色占满高度，下沿跟着卡片的圆角裁掉
      child: SizedBox.expand(
        child: Row(mainAxisAlignment: MainAxisAlignment.center, children: [
          ddIcon(icon, color: color),
          const SizedBox(width: 8),
          Text(label, style: TextStyle(fontSize: 14, color: color)),
        ]),
      ),
    );
  }
}


/// 设备详情下面的「设备信息」卡片（照原型 HomeDetail）：两列「名称 值」，只列网关给了值的项；IP 后面可以复制
class _DeviceInfo extends StatelessWidget {
  final DdWs ws;
  const _DeviceInfo({required this.ws});

  /// 网关给的时间（RFC3339）按本机时间显示成「年-月-日 时:分」
  static String _time(String s) {
    final t = DateTime.tryParse(s)?.toLocal();
    if (t == null) return s;
    String two(int n) => n.toString().padLeft(2, '0');
    return '${t.year}-${two(t.month)}-${two(t.day)} ${two(t.hour)}:${two(t.minute)}';
  }

  @override
  Widget build(BuildContext context) {
    final i = ws.info;
    final rows = <(String, String, bool)>[
      if (i['ip'] != null) (translate('IP address'), i['ip']!, true),
      // 网关给的系统是「类别 / 版本」（如 windows / Windows 11 Enterprise - 11 (26200)），只显示后面的版本
      if (ws.os.isNotEmpty) (translate('Operating system'), ws.os.split(' / ').last, false),
      if (i['hostname'] != null) (translate('Hostname'), i['hostname']!, false),
      if (i['os_username'] != null) (translate('Signed-in user'), i['os_username']!, false),
      if (i['cpu'] != null) ('CPU', i['cpu']!, false),
      if (i['memory'] != null) (translate('Memory'), i['memory']!, false),
      if (i['version'] != null) (translate('Version'), 'DreamDesk ${i['version']}', false),
      if (i['last_connected'] != null) (translate('Last connected'), _time(i['last_connected']!), false),
    ];
    if (rows.isEmpty) return const SizedBox.shrink();
    Widget label((String, String, bool) r) =>
        Padding(padding: const EdgeInsets.only(right: 12), child: Text(r.$1, style: const TextStyle(fontSize: 13, color: DdColors.ink2)));
    Widget value((String, String, bool) r) => Row(children: [
          Flexible(child: Text(r.$2, overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 13, color: DdColors.ink))),
          if (r.$3)
            TextButton(
              style: TextButton.styleFrom(
                  minimumSize: const Size(0, 20), padding: const EdgeInsets.symmetric(horizontal: 4), foregroundColor: DdColors.accent),
              onPressed: () {
                Clipboard.setData(ClipboardData(text: r.$2));
                showToast(translate('Copied'));
              },
              child: Text(translate('Copy'), style: const TextStyle(fontSize: 13)),
            ),
        ]);
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 18),
      decoration: ddCardDecoration(),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(translate('Device information'), style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600, color: DdColors.ink)),
        const SizedBox(height: 12),
        // 两列，按行往下排；窗口窄（默认 800 宽）时两列放不下 IP，改成一列。
        // 名称列至少 72 宽（照原型），英文名称更长时按最长的那个放宽，同一列的值对齐
        LayoutBuilder(builder: (context, c) {
          final cols = c.maxWidth >= 560 ? 2 : 1;
          const labelWidth = MaxColumnWidth(FixedColumnWidth(72 + 12), IntrinsicColumnWidth());
          final gap = TableRow(children: [for (var j = 0; j < (cols == 2 ? 5 : 2); j++) const SizedBox(height: 12)]);
          return Table(
            defaultVerticalAlignment: TableCellVerticalAlignment.middle,
            columnWidths: cols == 2
                ? const {0: labelWidth, 1: FlexColumnWidth(), 2: FixedColumnWidth(32), 3: labelWidth, 4: FlexColumnWidth()}
                : const {0: labelWidth, 1: FlexColumnWidth()},
            children: [
              for (var k = 0; k < rows.length; k += cols) ...[
                if (k > 0) gap,
                TableRow(children: [
                  label(rows[k]),
                  value(rows[k]),
                  if (cols == 2) ...[
                    const SizedBox.shrink(),
                    if (k + 1 < rows.length) ...[label(rows[k + 1]), value(rows[k + 1])] else ...[
                      const SizedBox.shrink(),
                      const SizedBox.shrink(),
                    ],
                  ],
                ]),
              ],
            ],
          );
        }),
      ]),
    );
  }
}

/// 全部设备里的一张：上面按系统画的主题背景，下面系统图标、名字和状态
class _DeviceTile extends StatelessWidget {
  final DdWs ws;
  final String stateText;
  final Color stateColor;
  /// 这台就是本机：名字后面挂「本机」
  final bool self;
  final VoidCallback onTap;
  const _DeviceTile(
      {required this.ws, required this.stateText, required this.stateColor, required this.self, required this.onTap});

  @override
  Widget build(BuildContext context) {
    // 状态行照原型：彩色圆点 + 状态；能连的后面再写系统（「在线 · Windows 11」）
    final os = ddOsShort(ws.os);
    final state = ws.selectable && os.isNotEmpty ? '$stateText · $os' : stateText;
    return Container(
      width: 220,
      decoration: BoxDecoration(borderRadius: BorderRadius.circular(DdRadius.lg), boxShadow: DdShadow.card),
      child: Material(
        color: DdColors.surface,
        borderRadius: BorderRadius.circular(DdRadius.lg),
        elevation: 0,
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            AspectRatio(
              aspectRatio: 2.2,
              child: _OsWallpaper(os: ws.os, online: ws.online),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
              child: Row(children: [
                _OsBadge(os: ws.os, online: ws.online, size: 20),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    Row(children: [
                      Flexible(
                        child: Text(_title(ws),
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 14, fontWeight: FontWeight.w600, color: ws.online ? DdColors.ink : DdColors.ink3)),
                      ),
                      if (self) DdTag(text: translate('This computer')),
                    ]),
                    const SizedBox(height: 2),
                    Row(children: [
                      DdDot(color: stateColor),
                      const SizedBox(width: 5),
                      Flexible(
                        child: Text(state,
                            overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 13, color: DdColors.ink2)),
                      ),
                    ]),
                  ]),
                ),
              ]),
            ),
          ]),
        ),
      ),
    );
  }
}

/// 没登录时的插画：一台显示器 + 一部手机，屏幕里是缩略的本程序界面（照 UU 的构图，用色块画）。
/// 这里和 [_DrawnWallpaper] 里的色值（紫、浅蓝、深蓝渐变、浅灰头像等）只给插画 / 画出来的壁纸用，不进 DdColors
class _Illustration extends StatelessWidget {
  const _Illustration();

  @override
  Widget build(BuildContext context) {
    Widget screen({required double w, required double h, required double r, required Widget child}) => Container(
          width: w,
          height: h,
          padding: EdgeInsets.all(r * 0.6),
          decoration: BoxDecoration(
            color: DdColors.ink,
            borderRadius: BorderRadius.circular(r),
            boxShadow: const [BoxShadow(color: Color(0x22000000), blurRadius: 18, offset: Offset(0, 8))],
          ),
          child: ClipRRect(borderRadius: BorderRadius.circular(r * 0.4), child: child),
        );
    Widget bar(double w, Color c) =>
        Container(width: w, height: 5, margin: const EdgeInsets.only(bottom: 5), decoration: BoxDecoration(color: c, borderRadius: BorderRadius.circular(3)));
    final desk = screen(
      w: 280,
      h: 180,
      r: 8,
      child: Row(children: [
        Container(
          width: 64,
          color: DdColors.chrome,
          padding: const EdgeInsets.fromLTRB(8, 12, 8, 8),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            bar(40, DdColors.ink3),
            bar(30, DdColors.accent),
            bar(36, DdColors.ink3),
            bar(26, DdColors.ink3),
          ]),
        ),
        Expanded(
          child: Container(
            color: DdColors.pane,
            padding: const EdgeInsets.all(10),
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              bar(60, DdColors.ink),
              const SizedBox(height: 4),
              Expanded(
                child: ClipRRect(
                  borderRadius: BorderRadius.circular(4),
                  child: const _OsWallpaper(os: 'windows', online: true),
                ),
              ),
              const SizedBox(height: 6),
              Row(children: [for (var i = 0; i < 4; i++) Expanded(child: bar(20, DdColors.line))]),
            ]),
          ),
        ),
      ]),
    );
    final phone = screen(
      w: 74,
      h: 140,
      r: 12,
      child: Container(
        color: DdColors.pane,
        padding: const EdgeInsets.all(6),
        child: Column(children: [
          bar(30, DdColors.ink),
          SizedBox(height: 40, child: ClipRRect(borderRadius: BorderRadius.circular(3), child: const _OsWallpaper(os: 'windows', online: true))),
          const SizedBox(height: 6),
          Wrap(spacing: 5, runSpacing: 5, children: [
            for (final c in [DdColors.accent, DdColors.osLinux, DdColors.online, const Color(0xFF7FA8FA), DdColors.ink3, DdColors.accent])
              Container(width: 14, height: 14, decoration: BoxDecoration(color: c, borderRadius: BorderRadius.circular(4))),
          ]),
        ]),
      ),
    );
    return SizedBox(
      width: 380,
      height: 230,
      child: Stack(children: [
        Positioned(left: 20, top: 30, child: desk),
        Positioned(right: 20, top: 70, child: phone),
        Positioned(left: 4, top: 18, child: _Spark(color: DdColors.osLinux, size: 12)),
        Positioned(right: 54, top: 10, child: _Spark(color: DdColors.accent, size: 10)),
        Positioned(right: 0, top: 120, child: _Spark(color: const Color(0xFF8B5CF6), size: 9)),
      ]),
    );
  }
}

class _Spark extends StatelessWidget {
  final Color color;
  final double size;
  const _Spark({required this.color, required this.size});

  @override
  Widget build(BuildContext context) => Transform.rotate(
        angle: math.pi / 4,
        child: Container(width: size, height: size, decoration: BoxDecoration(color: color, borderRadius: BorderRadius.circular(2))),
      );
}
