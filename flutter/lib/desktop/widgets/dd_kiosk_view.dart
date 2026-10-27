// DreamDesk kiosk 页面的画面（dreamdesk/docs/K-kiosk方案.md「界面」）：视觉照旧版 DreamDesk kiosk 界面——深色底、红色霓虹、
// 菱形标识、四角角标卡片、准星过场，流程也照旧版：登录卡片 → 选工作站 → 连接。按 1280×720 的设计尺寸摆放、整体缩放到屏幕
// （各种分辨率看着一样），背景铺满整屏。只管画，状态和动作都由 dd_kiosk_page.dart 给。背景静止不动；只有连接过场的准星在动，
// 过场一撤动画就停。
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter_hbb/desktop/widgets/dd_icons.dart';
import 'package:flutter_hbb/utils/dd_gateway.dart';

const kDdKioskRed = Color(0xFFFF2A3D);
const _bg = Color(0xFF05050A);
const _green = Color(0xFF22D27D);
const _teal = Color(0xFF2DD4BF);
// 中文写明用 Noto Sans CJK（kiosk 安装包依赖 fonts-noto-cjk）：不写的话跟着系统默认字体走，各台机器上的字可能不一样
const _cjk = 'Noto Sans CJK SC';

Color _white(double a) => Colors.white.withOpacity(a);

FontWeight _fw(double w) => FontWeight.values[((w / 100).round() - 1).clamp(0, 8)];

TextStyle _grotesk(double size,
        {double weight = 500, Color color = Colors.white, double em = 0, double? height, List<Shadow>? shadows}) =>
    TextStyle(
        fontFamily: 'DdSpaceGrotesk',
        fontFamilyFallback: const [_cjk],
        fontSize: size,
        fontWeight: _fw(weight),
        fontVariations: [FontVariation('wght', weight)],
        letterSpacing: em * size,
        height: height,
        color: color,
        shadows: shadows);

TextStyle _mono(double size, {Color? color, double em = 0}) => TextStyle(
    fontFamily: 'DdJetBrainsMono',
    fontFamilyFallback: const [_cjk],
    fontSize: size,
    letterSpacing: em * size,
    color: color ?? _white(0.6));

TextStyle _body(double size, {double weight = 400, Color? color, double? height}) => TextStyle(
    fontFamily: _cjk, fontSize: size, fontWeight: _fw(weight), height: height, color: color ?? Colors.white);

/// 整屏：背景铺满，内容放在 1280×720 的设计框里按屏幕缩放
class DdKioskFrame extends StatelessWidget {
  final List<Widget> layers;
  const DdKioskFrame({Key? key, required this.layers}) : super(key: key);

  @override
  Widget build(BuildContext context) {
    // 最外层要有 Material：没有它文字没有默认样式（Flutter 会给每段字画黄色双下划线），输入框也要它
    return Material(
      color: _bg,
      child: Stack(children: [
        // 旧版的两团光：画面中上一点偏紫、底部偏红。网格地平线、星点这版先不加（用户定）
        const Positioned.fill(
            child: DecoratedBox(
                decoration: BoxDecoration(
                    gradient: RadialGradient(
                        center: Alignment(0, -0.24),
                        radius: 0.9,
                        colors: [Color(0x8C280C1E), Color(0x0005050A)],
                        stops: [0, 0.6])))),
        const Positioned.fill(
            child: DecoratedBox(
                decoration: BoxDecoration(
                    gradient: RadialGradient(
                        center: Alignment(0, 1.05),
                        radius: 0.85,
                        colors: [Color(0x2EFF2A3D), Color(0x00FF2A3D)],
                        stops: [0, 0.6])))),
        // 要给缩放控件一个铺满整屏的固定框：放在 Center 里它只拿到「最多这么大」，就按设计尺寸原样摆着、不放大
        Positioned.fill(
          child: FittedBox(
            child: SizedBox(width: 1280, height: 720, child: Stack(children: layers)),
          ),
        ),
      ]),
    );
  }
}

/// 菱形标识：外框加实心芯，都转 45 度
class _Diamond extends StatelessWidget {
  const _Diamond();

  @override
  Widget build(BuildContext context) {
    const size = 20.0;
    return SizedBox(
      width: size,
      height: size,
      child: Stack(children: [
        Positioned.fill(
          child: Transform.rotate(
            angle: math.pi / 4,
            child: Container(
              decoration: BoxDecoration(
                border: Border.all(color: kDdKioskRed, width: size * 0.075),
                boxShadow: [BoxShadow(color: kDdKioskRed.withOpacity(0.6), blurRadius: size / 2)],
              ),
            ),
          ),
        ),
        Positioned.fill(
          child: Padding(
            padding: const EdgeInsets.all(size / 4),
            child: Transform.rotate(angle: math.pi / 4, child: Container(color: kDdKioskRed)),
          ),
        ),
      ]),
    );
  }
}

/// 顶栏：标识、标语；右边版本号、登录后的用户（点开有「退出登录」）、中 / EN
class DdKioskTopBar extends StatelessWidget {
  final String tagline;
  final String version;
  final bool zh;
  final void Function(bool zh) onLang;
  final String? user;
  final String signedInAs;
  final String signOut;
  final bool userMenuOpen;
  final VoidCallback? onUserTap;
  final VoidCallback? onSignOut;
  const DdKioskTopBar({
    Key? key,
    required this.tagline,
    required this.version,
    required this.zh,
    required this.onLang,
    this.user,
    this.signedInAs = '',
    this.signOut = '',
    this.userMenuOpen = false,
    this.onUserTap,
    this.onSignOut,
  }) : super(key: key);

  @override
  Widget build(BuildContext context) {
    final u = user;
    return Positioned(
      top: 28,
      left: 40,
      right: 40,
      child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
        SizedBox(
          height: 30,
          child: Row(children: [
            const _Diamond(),
            const SizedBox(width: 12),
            Text.rich(TextSpan(children: [
              TextSpan(text: 'Dream', style: _grotesk(15, weight: 700, em: -0.01)),
              TextSpan(text: 'Desk', style: _grotesk(15, weight: 400, color: kDdKioskRed)),
            ])),
            const SizedBox(width: 12),
            Container(width: 1, height: 16, color: _white(0.16)),
            const SizedBox(width: 12),
            Text(tagline.toUpperCase(), style: _mono(10, em: 0.18)),
          ]),
        ),
        const Spacer(),
        SizedBox(
          height: 30,
          child: Center(
              child: version.isEmpty
                  ? null
                  : Text('v$version', style: _mono(10, color: _white(0.55), em: 0.14))),
        ),
        if (u != null) ...[
          const SizedBox(width: 16),
          _UserChip(
              name: u,
              open: userMenuOpen,
              signedInAs: signedInAs,
              signOut: signOut,
              onTap: onUserTap,
              onSignOut: onSignOut),
        ],
        const SizedBox(width: 16),
        Container(
          decoration: BoxDecoration(border: Border.all(color: _white(0.18))),
          child: Row(mainAxisSize: MainAxisSize.min, children: [
            _LangCell(label: '中', active: zh, style: _body(11), onTap: () => onLang(true)),
            _LangCell(label: 'EN', active: !zh, style: _mono(11), onTap: () => onLang(false)),
          ]),
        ),
      ]),
    );
  }
}

class _LangCell extends StatelessWidget {
  final String label;
  final bool active;
  final TextStyle style;
  final VoidCallback onTap;
  const _LangCell({required this.label, required this.active, required this.style, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: active ? null : onTap,
        child: Container(
          height: 28,
          constraints: const BoxConstraints(minWidth: 40),
          alignment: Alignment.center,
          color: active ? kDdKioskRed.withOpacity(0.85) : Colors.transparent,
          child: Text(label, style: style.copyWith(color: active ? Colors.white : _white(0.7))),
        ),
      ),
    );
  }
}

/// 旧版的用户标签：首字方块 + 账号 + 下拉箭头；点开是「已登录 xxx / 退出登录」
class _UserChip extends StatelessWidget {
  final String name;
  final bool open;
  final String signedInAs;
  final String signOut;
  final VoidCallback? onTap;
  final VoidCallback? onSignOut;
  const _UserChip(
      {required this.name, required this.open, required this.signedInAs, required this.signOut, this.onTap, this.onSignOut});

  @override
  Widget build(BuildContext context) {
    final mono = name.replaceAll(RegExp(r'[^a-zA-Z一-鿿]'), '');
    final initial = mono.isEmpty ? '?' : mono.substring(0, 1).toUpperCase();
    return IntrinsicWidth(
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        MouseRegion(
          cursor: SystemMouseCursors.click,
          child: GestureDetector(
            onTap: onTap,
            child: Container(
              height: 30,
              padding: const EdgeInsets.fromLTRB(3, 0, 10, 0),
              decoration: BoxDecoration(border: Border.all(color: open ? kDdKioskRed : _white(0.18))),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                Container(
                  width: 22,
                  height: 22,
                  alignment: Alignment.center,
                  color: kDdKioskRed,
                  child: Text(initial, style: _grotesk(10, weight: 700)),
                ),
                const SizedBox(width: 8),
                Text(name, style: _body(11)),
                const SizedBox(width: 8),
                ddIcon(open ? DdIcon.chevronUp : DdIcon.chevronDown, size: 14, color: _white(0.8)),
              ]),
            ),
          ),
        ),
        if (open)
          Container(
            margin: const EdgeInsets.only(top: 6),
            decoration: BoxDecoration(
              color: const Color(0xF505050A),
              border: Border.all(color: kDdKioskRed),
              boxShadow: [BoxShadow(color: kDdKioskRed.withOpacity(0.25), blurRadius: 24)],
            ),
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              Container(
                padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
                decoration: BoxDecoration(border: Border(bottom: BorderSide(color: _white(0.12)))),
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(signedInAs.toUpperCase(), style: _mono(9, color: _white(0.55), em: 0.22)),
                  const SizedBox(height: 4),
                  Text(name, style: _body(12)),
                ]),
              ),
              MouseRegion(
                cursor: SystemMouseCursors.click,
                child: GestureDetector(
                  onTap: onSignOut,
                  child: Container(
                    color: kDdKioskRed.withOpacity(0.08),
                    padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
                    child: Row(children: [
                      ddIcon(DdIcon.logOut, size: 14, color: kDdKioskRed),
                      const SizedBox(width: 8),
                      Text(signOut, style: _body(12, color: kDdKioskRed)),
                    ]),
                  ),
                ),
              ),
            ]),
          ),
      ]),
    );
  }
}

/// 底栏：左边本机设备号、服务器（用户打电话给 IT 时报这几项），右边关机按钮（有的话）和版权
class DdKioskFooter extends StatelessWidget {
  final List<String> items;
  final Widget? trailing;
  const DdKioskFooter({Key? key, required this.items, this.trailing}) : super(key: key);

  @override
  Widget build(BuildContext context) {
    final style = _mono(10, em: 0.12);
    final t = trailing;
    return Positioned(
      left: 40,
      right: 40,
      bottom: 26,
      child: Row(children: [
        Container(
          width: 6,
          height: 6,
          decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: kDdKioskRed,
              boxShadow: [BoxShadow(color: kDdKioskRed.withOpacity(0.8), blurRadius: 8)]),
        ),
        const SizedBox(width: 8),
        for (final (i, s) in items.indexed) ...[
          if (i > 0) const SizedBox(width: 22),
          Text(s, style: style),
        ],
        const Spacer(),
        if (t != null) ...[t, const SizedBox(width: 18)],
        Text('© ${DateTime.now().year} 江苏原力数字科技股份有限公司', style: style),
      ]),
    );
  }
}

/// 四个角上的红色角标，盖在 child 外框上
class _Ticks extends StatelessWidget {
  final Widget child;
  final double len;
  const _Ticks({required this.child, this.len = 12});

  Widget _tick({bool top = false, bool left = false}) {
    const side = BorderSide(color: kDdKioskRed, width: 2);
    return Positioned(
      top: top ? -1 : null,
      bottom: top ? null : -1,
      left: left ? -1 : null,
      right: left ? null : -1,
      child: Container(
        width: len,
        height: len,
        decoration: BoxDecoration(
          border: Border(
            top: top ? side : BorderSide.none,
            bottom: top ? BorderSide.none : side,
            left: left ? side : BorderSide.none,
            right: left ? BorderSide.none : side,
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Stack(clipBehavior: Clip.none, children: [
      child,
      _tick(top: true, left: true),
      _tick(top: true),
      _tick(left: true),
      _tick(),
    ]);
  }
}

BoxDecoration _cardDecoration() => BoxDecoration(
      color: const Color(0xED08080E),
      border: Border.all(color: _white(0.16)),
      boxShadow: [
        BoxShadow(color: kDdKioskRed.withOpacity(0.15), spreadRadius: 1),
        const BoxShadow(color: Color(0x99000000), blurRadius: 50, offset: Offset(0, 20)),
      ],
    );

/// 卡片上「工作站」那一栏。online 为 null 时不显示在线状态（不知道）
class DdKioskTarget {
  final String label;
  final String name;
  final bool? online;
  final String onlineText;
  final String offlineText;
  const DdKioskTarget(this.label, this.name, this.online, this.onlineText, this.offlineText);
}

class DdKioskCountdown {
  final String text;
  final String attempts;
  final double fraction;
  const DdKioskCountdown(this.text, this.attempts, this.fraction);
}

/// 连接出错 / 断开后的状态卡片：出了什么事（标题）、原因、会不会自己重试（倒计时）、两个按钮
class DdKioskCard extends StatelessWidget {
  final String kicker;
  final String title;
  final String reason;
  final DdKioskTarget? target;
  final DdKioskCountdown? countdown;
  final String cta;
  final VoidCallback onCta;
  final String secondaryLabel;
  final DdIcon? secondaryIcon;
  final VoidCallback onSecondary;

  const DdKioskCard({
    Key? key,
    required this.kicker,
    required this.title,
    required this.reason,
    this.target,
    this.countdown,
    required this.cta,
    required this.onCta,
    required this.secondaryLabel,
    this.secondaryIcon,
    required this.onSecondary,
  }) : super(key: key);

  @override
  Widget build(BuildContext context) {
    final t = target;
    final c = countdown;
    return Align(
      alignment: const Alignment(0, 0.06),
      child: SizedBox(
        width: 560,
        child: _Ticks(
          child: Container(
            padding: const EdgeInsets.fromLTRB(38, 34, 38, 28),
            decoration: _cardDecoration(),
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
              Text('◢ ${kicker.toUpperCase()} ◣', style: _mono(10, color: kDdKioskRed, em: 0.42)),
              const SizedBox(height: 14),
              Text(title, style: _grotesk(30, weight: 600, em: -0.01, height: 1.2)),
              if (reason.isNotEmpty) ...[
                const SizedBox(height: 12),
                Text(reason, style: _body(14, color: _white(0.72), height: 1.65)),
              ],
              if (t != null) ...[
                const SizedBox(height: 22),
                Container(
                  padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
                  decoration: BoxDecoration(color: _white(0.02), border: Border.all(color: _white(0.12))),
                  child: Row(children: [
                    ddIcon(DdIcon.monitor, size: 18, color: _white(0.6)),
                    const SizedBox(width: 12),
                    Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                      Text(t.label.toUpperCase(), style: _mono(9, color: _white(0.55), em: 0.22)),
                      const SizedBox(height: 3),
                      Text(t.name, style: _body(14, weight: 500)),
                    ]),
                    const Spacer(),
                    if (t.online != null)
                      _StatusDot(color: t.online! ? _green : _white(0.4), text: t.online! ? t.onlineText : t.offlineText),
                  ]),
                ),
              ],
              if (c != null) ...[
                const SizedBox(height: 22),
                Row(children: [
                  Text(c.text, style: _mono(10, color: _white(0.65), em: 0.14)),
                  const Spacer(),
                  Text(c.attempts, style: _mono(10, color: _white(0.65), em: 0.14)),
                ]),
                const SizedBox(height: 8),
                Container(
                  height: 2,
                  color: _white(0.08),
                  alignment: Alignment.centerLeft,
                  // 每秒跳一格时连着走：一秒的动画接上一秒
                  child: AnimatedFractionallySizedBox(
                    duration: const Duration(seconds: 1),
                    widthFactor: c.fraction.clamp(0.0, 1.0),
                    child: Container(color: kDdKioskRed),
                  ),
                ),
              ],
              const SizedBox(height: 26),
              Row(children: [
                Expanded(child: _CtaButton(label: cta, onTap: onCta)),
                const SizedBox(width: 12),
                DdKioskGhostButton(
                    label: secondaryLabel, icon: secondaryIcon, width: 150, height: 52, onTap: onSecondary),
              ]),
              const SizedBox(height: 14),
              Center(child: Text('ENTER ↵  ${cta.toUpperCase()}', style: _mono(9, color: _white(0.5), em: 0.2))),
            ]),
          ),
        ),
      ),
    );
  }
}

class _StatusDot extends StatelessWidget {
  final Color color;
  final String text;
  const _StatusDot({required this.color, required this.text});

  @override
  Widget build(BuildContext context) {
    final glow = color != _white(0.4);
    return Row(mainAxisSize: MainAxisSize.min, children: [
      Container(
        width: 7,
        height: 7,
        decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: color,
            boxShadow: glow ? [BoxShadow(color: color.withOpacity(0.8), blurRadius: 8)] : null),
      ),
      const SizedBox(width: 8),
      Text(text.toUpperCase(), style: _mono(10, color: glow ? color : _white(0.6), em: 0.14)),
    ]);
  }
}

/// 主按钮：红底、四角白色角标，悬停时光晕变强；busy 时换成转圈、点了不响应
class _CtaButton extends StatefulWidget {
  final String label;
  final VoidCallback? onTap;
  final double height;
  final bool busy;
  const _CtaButton({required this.label, required this.onTap, this.height = 52, this.busy = false});

  @override
  State<_CtaButton> createState() => _CtaButtonState();
}

class _CtaButtonState extends State<_CtaButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    Widget tick(double? l, double? t, double? r, double? b) {
      const s = BorderSide(color: Colors.white, width: 1.5);
      return Positioned(
        left: l,
        top: t,
        right: r,
        bottom: b,
        child: Container(
          width: 7,
          height: 7,
          decoration: BoxDecoration(
              border: Border(
                  left: l != null ? s : BorderSide.none,
                  right: r != null ? s : BorderSide.none,
                  top: t != null ? s : BorderSide.none,
                  bottom: b != null ? s : BorderSide.none)),
        ),
      );
    }

    final enabled = widget.onTap != null && !widget.busy;
    final hover = _hover && enabled;
    return MouseRegion(
      cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: enabled ? widget.onTap : null,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 200),
          height: widget.height,
          decoration: BoxDecoration(
            color: kDdKioskRed.withOpacity(!enabled ? 0.35 : (hover ? 0.92 : 0.82)),
            boxShadow: [
              BoxShadow(color: kDdKioskRed.withOpacity(0.7), spreadRadius: 1),
              BoxShadow(color: kDdKioskRed.withOpacity(hover ? 0.55 : 0.32), blurRadius: hover ? 30 : 18),
              BoxShadow(color: kDdKioskRed.withOpacity(hover ? 0.32 : 0.18), blurRadius: hover ? 80 : 50),
            ],
          ),
          child: Stack(children: [
            tick(9, 9, null, null),
            tick(null, 9, 9, null),
            tick(9, null, null, 9),
            tick(null, null, 9, 9),
            Center(
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                Flexible(
                    child: Text(widget.label,
                        overflow: TextOverflow.ellipsis, style: _grotesk(16, weight: 600, em: 0.06))),
                const SizedBox(width: 12),
                if (widget.busy)
                  const SizedBox(
                      width: 14, height: 14, child: CircularProgressIndicator(strokeWidth: 1.5, color: Colors.white))
                else
                  AnimatedSlide(
                    duration: const Duration(milliseconds: 200),
                    offset: Offset(hover ? 0.4 : 0, 0),
                    child: ddIcon(DdIcon.arrowRight, size: 18, color: Colors.white),
                  ),
              ]),
            ),
          ]),
        ),
      ),
    );
  }
}

class DdKioskGhostButton extends StatelessWidget {
  final String label;
  final DdIcon? icon;
  final double? width;
  final double height;
  final double fontSize;
  final VoidCallback onTap;
  const DdKioskGhostButton(
      {Key? key, required this.label, this.icon, this.width, this.height = 28, this.fontSize = 15, required this.onTap})
      : super(key: key);

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          width: width,
          height: height,
          padding: const EdgeInsets.symmetric(horizontal: 14),
          decoration: BoxDecoration(border: Border.all(color: _white(0.24))),
          child: Row(mainAxisSize: MainAxisSize.min, mainAxisAlignment: MainAxisAlignment.center, children: [
            if (icon != null) ...[ddIcon(icon!, size: fontSize + 1, color: _white(0.88)), const SizedBox(width: 8)],
            Text(label, style: _grotesk(fontSize, weight: 500, color: _white(0.88))),
          ]),
        ),
      ),
    );
  }
}

/// 一行输入框：浅色底块里图标加输入，空着时显示字段名；聚焦时底块提亮、图标变红（不画下划线，用户定）
class _Field extends StatefulWidget {
  final String label;
  final DdIcon icon;
  final TextEditingController controller;
  final FocusNode focusNode;
  final bool password;
  final bool mono;
  final ValueChanged<String> onSubmitted;
  const _Field({
    required this.label,
    required this.icon,
    required this.controller,
    required this.focusNode,
    this.password = false,
    this.mono = false,
    required this.onSubmitted,
  });

  @override
  State<_Field> createState() => _FieldState();
}

class _FieldState extends State<_Field> {
  // 口令框的明文切换：全屏 kiosk 上输错了只能全删重敲，没有它用户没法自查（旧版同样有）
  bool _reveal = false;

  @override
  void initState() {
    super.initState();
    widget.focusNode.addListener(_onFocus);
  }

  @override
  void dispose() {
    widget.focusNode.removeListener(_onFocus);
    super.dispose();
  }

  void _onFocus() => setState(() {});

  @override
  Widget build(BuildContext context) {
    final focus = widget.focusNode.hasFocus;
    return Container(
        height: 46,
        padding: const EdgeInsets.symmetric(horizontal: 14),
        color: _white(focus ? 0.09 : 0.05),
        child: Row(children: [
          ddIcon(widget.icon, size: 16, color: focus ? kDdKioskRed : _white(0.55)),
          const SizedBox(width: 10),
          Expanded(
            child: TextField(
              controller: widget.controller,
              focusNode: widget.focusNode,
              obscureText: widget.password && !_reveal,
              autocorrect: false,
              enableSuggestions: false,
              cursorColor: kDdKioskRed,
              style: widget.mono ? _mono(16, color: Colors.white) : _body(16, weight: 500),
              // 程序主题给输入框默认填了白底（配白字就看不清）：关掉，底色只用外面这块深色半透明
              decoration: InputDecoration(
                  filled: false,
                  hintText: widget.label,
                  hintStyle: _body(15, color: _white(0.4)),
                  isDense: true,
                  border: InputBorder.none,
                  focusedBorder: InputBorder.none,
                  enabledBorder: InputBorder.none,
                  contentPadding: const EdgeInsets.symmetric(vertical: 4)),
              onSubmitted: widget.onSubmitted,
            ),
          ),
          if (widget.password)
            MouseRegion(
              cursor: SystemMouseCursors.click,
              child: GestureDetector(
                onTap: () => setState(() => _reveal = !_reveal),
                child: ddIcon(_reveal ? DdIcon.eyeOff : DdIcon.eye,
                    size: 16, color: _reveal ? kDdKioskRed : _white(0.55)),
              ),
            ),
        ]));
  }
}

class DdKioskLoginTexts {
  final String kicker;
  final String title;
  final String user;
  final String password;
  final String submit;
  const DdKioskLoginTexts(
      {required this.kicker, required this.title, required this.user, required this.password, required this.submit});
}

/// 旧版的登录卡片：用户名、口令（可切明文）、登录失败的原因、登录按钮。口令只在这个控件里，提交后由页面清掉
class DdKioskLogin extends StatefulWidget {
  final DdKioskLoginTexts texts;
  final String initialUser;
  final bool busy;
  final String error;
  final void Function(String user, String password) onSubmit;
  const DdKioskLogin({
    Key? key,
    required this.texts,
    required this.initialUser,
    required this.busy,
    required this.error,
    required this.onSubmit,
  }) : super(key: key);

  @override
  State<DdKioskLogin> createState() => DdKioskLoginState();
}

class DdKioskLoginState extends State<DdKioskLogin> {
  late final _user = TextEditingController(text: widget.initialUser);
  final _password = TextEditingController();
  final _userFocus = FocusNode();
  final _passwordFocus = FocusNode();

  @override
  void initState() {
    super.initState();
    // 记得上次的用户名时直接落到口令框
    WidgetsBinding.instance.addPostFrameCallback(
        (_) => (widget.initialUser.isEmpty ? _userFocus : _passwordFocus).requestFocus());
  }

  @override
  void dispose() {
    _user.dispose();
    _password.dispose();
    _userFocus.dispose();
    _passwordFocus.dispose();
    super.dispose();
  }

  /// 登录失败后清掉口令、焦点回到口令框；闲置太久也清
  void clearPassword() {
    _password.clear();
    _passwordFocus.requestFocus();
  }

  void _submit() {
    if (widget.busy) return;
    final u = _user.text.trim();
    if (u.isEmpty) {
      _userFocus.requestFocus();
      return;
    }
    if (_password.text.isEmpty) {
      _passwordFocus.requestFocus();
      return;
    }
    widget.onSubmit(u, _password.text);
  }

  @override
  Widget build(BuildContext context) {
    final t = widget.texts;
    return Align(
      alignment: const Alignment(0, 0.04),
      child: SizedBox(
        width: 400,
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          Text('◢ ${t.kicker.toUpperCase()} ◣', style: _mono(10, color: kDdKioskRed, em: 0.42)),
          const SizedBox(height: 10),
          Text.rich(TextSpan(children: [
            TextSpan(text: '${t.title} Dream', style: _grotesk(32, em: -0.02)),
            TextSpan(text: 'Desk', style: _grotesk(32, em: -0.02, color: kDdKioskRed)),
          ])),
          const SizedBox(height: 26),
          _Ticks(
            len: 10,
            child: Container(
              padding: const EdgeInsets.fromLTRB(30, 28, 30, 22),
              decoration: _cardDecoration(),
              child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: [
                _Field(
                    label: t.user,
                    icon: DdIcon.user,
                    controller: _user,
                    focusNode: _userFocus,
                    onSubmitted: (_) => _passwordFocus.requestFocus()),
                const SizedBox(height: 14),
                _Field(
                    label: t.password,
                    icon: DdIcon.lock,
                    controller: _password,
                    focusNode: _passwordFocus,
                    password: true,
                    mono: true,
                    onSubmitted: (_) => _submit()),
                if (widget.error.isNotEmpty) ...[
                  const SizedBox(height: 14),
                  Container(
                    padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
                    color: kDdKioskRed.withOpacity(0.12),
                    child: Text(widget.error, style: _body(13, height: 1.4)),
                  ),
                ],
                const SizedBox(height: 20),
                _CtaButton(label: t.submit, height: 48, busy: widget.busy, onTap: _submit),
              ]),
            ),
          ),
        ]),
      ),
    );
  }
}

class DdKioskWsTexts {
  final String kicker;
  final String titleA;
  final String titleB;
  final String sub;
  final String Function(DdWsState) stateText;
  final String connect;
  final String hint;
  const DdKioskWsTexts(
      {required this.kicker,
      required this.titleA,
      required this.titleB,
      required this.sub,
      required this.stateText,
      required this.connect,
      required this.hint});
}

/// 旧版的选机页：标题、工作站卡片（状态点：绿 = 可以连接、红 = 别人在用、灰 = 离线；别人在用和离线的点不了）、连接按钮
class DdKioskWorkstations extends StatelessWidget {
  final DdKioskWsTexts texts;
  final List<DdWs> items;
  final String? selectedId;
  final ValueChanged<String> onSelect;
  final ValueChanged<String> onConnect;
  const DdKioskWorkstations({
    Key? key,
    required this.texts,
    required this.items,
    required this.selectedId,
    required this.onSelect,
    required this.onConnect,
  }) : super(key: key);

  Color _color(DdWsState s) => switch (s) {
        DdWsState.available || DdWsState.mine => _green,
        DdWsState.busy => kDdKioskRed,
        DdWsState.offline => _white(0.4),
      };

  @override
  Widget build(BuildContext context) {
    final t = texts;
    final selected = items.where((w) => w.id == selectedId && w.selectable).firstOrNull;
    return Stack(children: [
      Positioned(
        top: 96,
        left: 0,
        right: 0,
        child: Column(children: [
          Text('◢ ${t.kicker.toUpperCase()} ◣', style: _mono(10, color: kDdKioskRed, em: 0.42)),
          const SizedBox(height: 10),
          Text.rich(TextSpan(children: [
            TextSpan(text: t.titleA, style: _grotesk(34, em: -0.02)),
            TextSpan(text: t.titleB, style: _grotesk(34, em: -0.02, color: kDdKioskRed)),
          ])),
          const SizedBox(height: 8),
          Text(t.sub, style: _body(13, color: _white(0.65))),
        ]),
      ),
      Positioned(
        left: 120,
        right: 120,
        top: 226,
        height: 270,
        child: SingleChildScrollView(
          child: Wrap(spacing: 14, runSpacing: 14, children: [
            for (final (i, w) in items.indexed) _card(i, w, w.id == selectedId),
          ]),
        ),
      ),
      if (selected != null)
        Positioned(
          left: 400,
          right: 400,
          top: 520,
          child: Column(children: [
            _CtaButton(label: '${t.connect} ${selected.name}', height: 54, onTap: () => onConnect(selected.id)),
            const SizedBox(height: 12),
            Text(t.hint.toUpperCase(), style: _mono(9, color: _white(0.5), em: 0.2)),
          ]),
        ),
    ]);
  }

  Widget _card(int i, DdWs w, bool sel) {
    final color = _color(w.state);
    final on = w.selectable;
    return Opacity(
      opacity: on ? 1 : 0.5,
      child: MouseRegion(
        cursor: on ? SystemMouseCursors.click : SystemMouseCursors.forbidden,
        child: GestureDetector(
          onTap: on ? () => onSelect(w.id) : null,
          onDoubleTap: on ? () => onConnect(w.id) : null,
          child: Container(
            width: 249,
            height: 120,
            padding: const EdgeInsets.fromLTRB(16, 14, 16, 12),
            decoration: BoxDecoration(
              color: sel ? kDdKioskRed.withOpacity(0.08) : const Color(0x9908080E),
              border: Border.all(color: sel ? kDdKioskRed : _white(0.14)),
              boxShadow: sel ? [BoxShadow(color: kDdKioskRed.withOpacity(0.22), blurRadius: 22)] : null,
            ),
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                Text(i < 9 ? '${i + 1}' : '', style: _mono(10, color: _white(0.55), em: 0.2)),
                const Spacer(),
                Container(
                  width: 7,
                  height: 7,
                  decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: color,
                      boxShadow: w.state == DdWsState.offline
                          ? null
                          : [BoxShadow(color: color.withOpacity(0.8), blurRadius: 8)]),
                ),
              ]),
              const SizedBox(height: 10),
              Text(w.name,
                  maxLines: 1, overflow: TextOverflow.ellipsis, style: _grotesk(16, weight: 600)),
              const SizedBox(height: 3),
              Text(w.id, style: _mono(10, color: _white(0.55))),
              const Spacer(),
              Text(texts.stateText(w.state).toUpperCase(),
                  style: _mono(10, color: w.state == DdWsState.offline ? _white(0.6) : color, em: 0.12)),
            ]),
          ),
        ),
      ),
    );
  }
}

/// 连接过场：准星加放射线，下面一行「打开画面」在转。只显示真实发生的步骤，不编步骤
class DdKioskConnecting extends StatefulWidget {
  final String kicker;
  final String title;
  final String target;
  final String step;
  const DdKioskConnecting({
    Key? key,
    required this.kicker,
    required this.title,
    required this.target,
    required this.step,
  }) : super(key: key);

  @override
  State<DdKioskConnecting> createState() => _DdKioskConnectingState();
}

class _DdKioskConnectingState extends State<DdKioskConnecting> with SingleTickerProviderStateMixin {
  late final AnimationController _pulse =
      AnimationController(vsync: this, duration: const Duration(milliseconds: 800))..repeat(reverse: true);

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final w = widget;
    return Positioned.fill(
      child: Container(
        color: const Color(0xEB05050A),
        child: Stack(children: [
          const Positioned.fill(child: CustomPaint(painter: _WarpPainter())),
          Center(
            child: Column(mainAxisSize: MainAxisSize.min, children: [
              AnimatedBuilder(
                animation: _pulse,
                builder: (context, child) => Opacity(
                  opacity: 1 - 0.38 * _pulse.value,
                  child: Transform.scale(scale: 1 - 0.035 * _pulse.value, child: child),
                ),
                child: const SizedBox(width: 136, height: 136, child: CustomPaint(painter: _ReticlePainter())),
              ),
              const SizedBox(height: 22),
              Text('◢ ${w.kicker.toUpperCase()} ◣', style: _mono(11, color: kDdKioskRed, em: 0.42)),
              const SizedBox(height: 12),
              Text.rich(
                TextSpan(children: [
                  TextSpan(text: '${w.title} ', style: _grotesk(30, em: -0.01)),
                  TextSpan(
                      text: w.target,
                      style: _grotesk(30,
                          em: -0.01,
                          color: kDdKioskRed,
                          shadows: [Shadow(color: kDdKioskRed.withOpacity(0.9), blurRadius: 14)])),
                ]),
                textAlign: TextAlign.center,
              ),
              const SizedBox(height: 26),
              Row(mainAxisSize: MainAxisSize.min, children: [
                SizedBox(width: 10, height: 10, child: CircularProgressIndicator(strokeWidth: 1.3, color: _white(0.9))),
                const SizedBox(width: 8),
                Text(w.step, style: _mono(11, color: Colors.white, em: 0.12)),
              ]),
              const SizedBox(height: 66),
            ]),
          ),
        ]),
      ),
    );
  }
}

class _WarpPainter extends CustomPainter {
  const _WarpPainter();

  @override
  void paint(Canvas canvas, Size size) {
    final c = Offset(size.width / 2, size.height / 2 - 30);
    for (var i = 0; i < 60; i++) {
      final a = i / 60 * math.pi * 2 + (i.isOdd ? 0.04 : 0);
      final len = 100.0 + (i % 7) * 30;
      final d = Offset(math.cos(a), math.sin(a));
      final paint = Paint()
        ..strokeWidth = i % 5 == 0 ? 1.5 : 0.7
        ..color = (i % 5 == 0 ? kDdKioskRed : (i % 3 == 0 ? _teal : Colors.white))
            .withOpacity(i % 5 == 0 || i % 3 == 0 ? 0.35 : 0.18);
      canvas.drawLine(c + d * 90, c + d * (90 + len), paint);
    }
  }

  @override
  bool shouldRepaint(covariant CustomPainter oldDelegate) => false;
}

/// 准星：红外圈、虚线圈、青色内圈、中心点、十字线、四角角标
class _ReticlePainter extends CustomPainter {
  const _ReticlePainter();

  @override
  void paint(Canvas canvas, Size size) {
    final c = size.center(Offset.zero);
    const r = 60.0;
    canvas.drawCircle(
        c,
        r,
        Paint()
          ..color = kDdKioskRed.withOpacity(0.45)
          ..style = PaintingStyle.stroke
          ..strokeWidth = 4
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 8));
    canvas.drawCircle(
        c,
        r,
        Paint()
          ..color = kDdKioskRed
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.5);
    final dash = Paint()
      ..color = kDdKioskRed.withOpacity(0.6)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1;
    const segs = 36;
    for (var i = 0; i < segs; i++) {
      canvas.drawArc(Rect.fromCircle(center: c, radius: r - 15), i / segs * math.pi * 2, math.pi / segs, false, dash);
    }
    canvas.drawCircle(
        c,
        r - 35,
        Paint()
          ..color = _teal.withOpacity(0.5)
          ..style = PaintingStyle.stroke
          ..strokeWidth = 3
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 5));
    canvas.drawCircle(
        c,
        r - 35,
        Paint()
          ..color = _teal
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1);
    canvas.drawCircle(
        c,
        6,
        Paint()
          ..color = kDdKioskRed
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 6));
    canvas.drawCircle(c, 3, Paint()..color = kDdKioskRed);
    final cross = Paint()
      ..color = kDdKioskRed.withOpacity(0.7)
      ..strokeWidth = 1;
    canvas.drawLine(c.translate(0, -r - 6), c.translate(0, r + 6), cross);
    canvas.drawLine(c.translate(-r - 6, 0), c.translate(r + 6, 0), cross);
    final tick = Paint()
      ..color = kDdKioskRed
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2;
    const o = r + 8.0;
    const l = 9.0;
    for (final sx in [-1.0, 1.0]) {
      for (final sy in [-1.0, 1.0]) {
        final p = c.translate(sx * o, sy * o);
        canvas.drawPath(
            Path()
              ..moveTo(p.dx, p.dy - sy * l)
              ..lineTo(p.dx, p.dy)
              ..lineTo(p.dx - sx * l, p.dy),
            tick);
      }
    }
  }

  @override
  bool shouldRepaint(covariant CustomPainter oldDelegate) => false;
}

/// 连上以后主窗口在专业画面后面：只留一行字，不画动画
class DdKioskConnected extends StatelessWidget {
  final String text;
  const DdKioskConnected({Key? key, required this.text}) : super(key: key);

  @override
  Widget build(BuildContext context) =>
      Center(child: Text(text.toUpperCase(), style: _mono(11, color: _white(0.45), em: 0.2)));
}

/// 关机确认
class DdKioskConfirm extends StatelessWidget {
  final String title;
  final String text;
  final String confirmLabel;
  final String cancelLabel;
  final VoidCallback onConfirm;
  final VoidCallback onCancel;
  const DdKioskConfirm({
    Key? key,
    required this.title,
    required this.text,
    required this.confirmLabel,
    required this.cancelLabel,
    required this.onConfirm,
    required this.onCancel,
  }) : super(key: key);

  @override
  Widget build(BuildContext context) {
    return Positioned.fill(
      child: GestureDetector(
        onTap: onCancel,
        child: Container(
          color: const Color(0x9E000000),
          alignment: Alignment.center,
          child: GestureDetector(
            onTap: () {},
            child: SizedBox(
              width: 420,
              child: _Ticks(
                len: 10,
                child: Container(
                  padding: const EdgeInsets.fromLTRB(30, 28, 30, 24),
                  decoration: BoxDecoration(
                    color: const Color(0xF708080E),
                    border: Border.all(color: _white(0.18)),
                    boxShadow: const [BoxShadow(color: Color(0xB3000000), blurRadius: 60, offset: Offset(0, 20))],
                  ),
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
                    Text(title, style: _grotesk(20, weight: 600)),
                    const SizedBox(height: 10),
                    Text(text, style: _body(13, color: _white(0.72), height: 1.6)),
                    const SizedBox(height: 22),
                    Row(mainAxisAlignment: MainAxisAlignment.end, children: [
                      DdKioskGhostButton(label: cancelLabel, height: 40, fontSize: 14, onTap: onCancel),
                      const SizedBox(width: 10),
                      SizedBox(width: 110, child: _CtaButton(label: confirmLabel, height: 40, onTap: onConfirm)),
                    ]),
                  ]),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class DdKioskShuttingDown extends StatelessWidget {
  final String text;
  const DdKioskShuttingDown({Key? key, required this.text}) : super(key: key);

  @override
  Widget build(BuildContext context) => Positioned.fill(
        child: Container(
          color: Colors.black,
          alignment: Alignment.center,
          child: Text(text, style: _mono(12, color: _white(0.6), em: 0.3)),
        ),
      );
}
