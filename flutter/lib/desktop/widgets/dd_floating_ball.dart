// DreamDesk 通用窗口全屏时的悬浮球（dreamdesk/docs/M-受管版方案.md 第 3b 步 D，用户 10-26 定：全屏用悬浮球，和专业模式同一个球）：
// 全屏时标题栏收起，操作都从这个球进。外观和手感照专业窗口的悬浮球（src/ddpro/ball.rs，它又照旧 DreamDesk）：42 像素圆角方块，
// 常态浅灰底 + 浅灰带状边框，悬停转白底 + 蓝边；默认缩在顶边、离右边 220 像素（躲开被控端最大化窗口右上角的按钮），缩进时整块藏起来，
// 鼠标靠近那段边就滑出来；能拖，松手时离边近就贴过去，停在中间没人碰 1 秒就回到顶边。点开是控制中心那份菜单（「全屏」换成「退出全屏」）。
// 球和菜单盖在画面上面，鼠标在它们上面时点击不会落到远程。kiosk 的通用窗口始终全屏，也用它。
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/desktop/widgets/dd_icons.dart';
import 'package:flutter_hbb/desktop/widgets/dd_remote_title_bar.dart';
import 'package:flutter_hbb/models/model.dart';

const _kBall = 42.0;
const _kRadius = 13.0;
const _kBorder = 4.0;
/// 缩进时边上留多宽的感应带：鼠标进来就把球滑出来
const _kPeek = 15.0;
const _kRightGap = 220.0;
/// 松手时球心离边多近就贴边
const _kSnap = 42.0;
/// 感应带比球两边各宽出这么多
const _kReveal = 24.0;
const _kIdleDock = Duration(seconds: 1);
const _kSlide = Duration(milliseconds: 150);
// 配色照专业窗口的球
const _kBallBg = Color.fromARGB(247, 241, 241, 242);
const _kBallBgHot = Color.fromARGB(252, 255, 255, 255);
const _kBand = Color.fromARGB(255, 227, 227, 230);
const _kBandHot = Color.fromARGB(255, 177, 203, 255);
const _kBallFg = Color.fromARGB(255, 71, 77, 87);

enum _Edge { top, left, right }

class DdFloatingBall extends StatefulWidget {
  final String id;
  final FFI ffi;
  const DdFloatingBall({Key? key, required this.id, required this.ffi}) : super(key: key);

  @override
  State<DdFloatingBall> createState() => _DdFloatingBallState();
}

class _DdFloatingBallState extends State<DdFloatingBall> {
  /// 贴在哪条边、沿边的位置（顶边是左边距，左右边是上边距）；null 表示不在边上（拖到中间停住了），球在 [_free]
  _Edge? _edge = _Edge.top;
  double? _along;
  Offset _free = Offset.zero;
  bool _revealed = false;
  bool _hover = false;
  bool _dragging = false;
  bool _moved = false;
  Timer? _idle;
  Timer? _hide;
  final _menu = MenuController();
  /// 菜单开没开自己记（MenuAnchor 的 onOpen / onClose）：不能在画的时候问 `_menu.isOpen`——第一次画时菜单还没挂上，
  /// 一问就抛错，发布版把整个球（铺满画面的那一层）画成灰色的出错块，全屏画面蒙一层灰白
  bool _menuOpen = false;
  List<Widget> _children = const [];

  @override
  void dispose() {
    _idle?.cancel();
    _hide?.cancel();
    super.dispose();
  }

  bool get _busy => _hover || _dragging || _menuOpen;

  /// 鼠标离开、菜单关了一会儿后，贴边的球缩回去；停在中间的过 1 秒回到顶边
  void _scheduleTuck() {
    _hide?.cancel();
    _hide = Timer(const Duration(milliseconds: 400), () {
      if (!mounted || _busy) return;
      setState(() => _revealed = false);
    });
  }

  void _scheduleIdleDock() {
    _idle?.cancel();
    _idle = Timer(_kIdleDock, () {
      if (!mounted || _busy || _edge != null) return;
      setState(() {
        _edge = _Edge.top;
        _along = _free.dx;
        _revealed = false;
      });
    });
  }

  /// 限制在 0 到 max 之间；窗口比球还小（刚建起来还没量好尺寸）时 max 是负的，clamp 会抛错，这时取 0
  static double _fit(double v, double max) => v.clamp(0.0, max < 0 ? 0.0 : max);

  Offset _ballPos(Size size) {
    final along = _fit(_along ?? size.width - _kRightGap - _kBall,
        (_edge == _Edge.top ? size.width : size.height) - _kBall);
    final shown = _revealed || _busy;
    switch (_edge) {
      case _Edge.top:
        return Offset(along, shown ? 0 : -_kBall - 12);
      case _Edge.left:
        return Offset(shown ? 0 : -_kBall - 12, along);
      case _Edge.right:
        return Offset(shown ? size.width - _kBall : size.width + 12, along);
      case null:
        return Offset(_fit(_free.dx, size.width - _kBall), _fit(_free.dy, size.height - _kBall));
    }
  }

  /// 缩进时边上的感应带（只感应鼠标靠近，不拦点击：点击照样落到远程）
  Rect? _peekZone(Size size, Offset ball) {
    if (_edge == null || _revealed || _busy) return null;
    switch (_edge!) {
      case _Edge.top:
        return Rect.fromLTWH(ball.dx - _kReveal, 0, _kBall + 2 * _kReveal, _kPeek);
      case _Edge.left:
        return Rect.fromLTWH(0, ball.dy - _kReveal, _kPeek, _kBall + 2 * _kReveal);
      case _Edge.right:
        return Rect.fromLTWH(size.width - _kPeek, ball.dy - _kReveal, _kPeek, _kBall + 2 * _kReveal);
    }
  }

  void _onDragEnd(Size size) {
    final c = _free + const Offset(_kBall / 2, _kBall / 2);
    setState(() {
      _dragging = false;
      if (c.dy < _kSnap) {
        _edge = _Edge.top;
        _along = _free.dx;
      } else if (c.dx < _kSnap) {
        _edge = _Edge.left;
        _along = _free.dy;
      } else if (size.width - c.dx < _kSnap) {
        _edge = _Edge.right;
        _along = _free.dy;
      } else {
        _edge = null;
      }
      _revealed = true;
    });
    if (_edge == null) {
      _scheduleIdleDock();
    } else {
      _scheduleTuck();
    }
  }

  void _toggleMenu() {
    if (_menuOpen) {
      _menu.close();
      return;
    }
    setState(() => _children = ddControlMenu(context, widget.id, widget.ffi, fullscreen: true));
    _menu.open();
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(builder: (context, constraints) {
      final size = constraints.biggest;
      final ball = _ballPos(size);
      final zone = _peekZone(size, ball);
      return Stack(children: [
        if (zone != null)
          Positioned.fromRect(
            rect: zone,
            child: MouseRegion(
              opaque: false,
              onEnter: (_) => setState(() => _revealed = true),
            ),
          ),
        AnimatedPositioned(
          duration: _dragging ? Duration.zero : _kSlide,
          left: ball.dx,
          top: ball.dy,
          child: MenuAnchor(
            controller: _menu,
            style: ddMenuStyle,
            alignmentOffset: const Offset(0, 6),
            menuChildren: _children,
            onOpen: () => setState(() => _menuOpen = true),
            onClose: () {
              setState(() => _menuOpen = false);
              if (_edge == null) {
                _scheduleIdleDock();
              } else {
                _scheduleTuck();
              }
            },
            child: MouseRegion(
              cursor: SystemMouseCursors.click,
              onEnter: (_) => setState(() {
                _hover = true;
                _revealed = true;
              }),
              onExit: (_) {
                setState(() => _hover = false);
                if (_edge == null) {
                  _scheduleIdleDock();
                } else {
                  _scheduleTuck();
                }
              },
              child: GestureDetector(
                // 松开时才算点击（拖动过就不算），和专业窗口的球一样
                onPanStart: (_) => setState(() {
                  _dragging = true;
                  _moved = false;
                  _free = ball;
                  _edge = null;
                }),
                onPanUpdate: (d) => setState(() {
                  _moved = true;
                  _free = Offset(_fit(_free.dx + d.delta.dx, size.width - _kBall),
                      _fit(_free.dy + d.delta.dy, size.height - _kBall));
                }),
                onPanEnd: (_) {
                  if (_moved) {
                    _onDragEnd(size);
                  } else {
                    setState(() => _dragging = false);
                    _toggleMenu();
                  }
                },
                onTap: _toggleMenu,
                child: _BallFace(hot: _hover || _menuOpen || _dragging),
              ),
            ),
          ),
        ),
      ]);
    });
  }
}

class _BallFace extends StatelessWidget {
  final bool hot;
  const _BallFace({required this.hot});

  @override
  Widget build(BuildContext context) {
    return Container(
      width: _kBall,
      height: _kBall,
      decoration: BoxDecoration(
        color: hot ? _kBallBgHot : _kBallBg,
        borderRadius: BorderRadius.circular(_kRadius),
        border: Border.all(color: hot ? _kBandHot : _kBand, width: _kBorder),
        boxShadow: const [BoxShadow(color: Color(0x33000000), blurRadius: 10, offset: Offset(0, 2))],
      ),
      alignment: Alignment.center,
      // 图标照专业窗口的球：两条上下叠的胶囊，各带一个错开的小圆点
      child: ddIcon(DdIcon.controlCenter, size: 22, color: _kBallFg),
    );
  }
}
