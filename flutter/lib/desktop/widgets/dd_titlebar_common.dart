// 远程窗口（dd_remote_title_bar.dart）和文件传输 / 终端 / 端口映射 / 连接管理器窗口（dd_sub_windows.dart）的标题栏共用的部件：
// 拖动区（按住拖窗口、双击最大化）、圆角小按钮、窗口按钮、标签的外壳（尺寸和形状）、机器名。两处原来各写一份，改一处容易漏另一处。
import 'dart:convert';

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/shared_state.dart';
import 'package:flutter_hbb/main.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:get/get.dart';
import 'package:window_manager/window_manager.dart';

import 'dd_icons.dart';
import 'dd_theme.dart';
import 'tabbar_widget.dart';

/// 标签最宽 / 最窄（照 UU：内容靠左；原型 Remote.html「窗口缩放时标签渐进变窄」）
const kDdTabWidth = 230.0;
const kDdTabIconWidth = 40.0;

/// 标签窄于这个就只剩图标居中（放不下「图标 + 两三个字」），名字放悬停提示
const kDdTabTextMinWidth = 96.0;

/// 每个标签的宽：标签区能用的宽（减掉两边各 8 像素给选中标签的弯角）平分给 [count] 个标签，夹在最窄、最宽之间。
/// 夹到最窄还放不下时标签区横着滚
double ddTabWidthFor(double available, int count) {
  if (count <= 0 || !available.isFinite) return kDdTabWidth;
  return ((available - 16) / count).floorToDouble().clamp(kDdTabIconWidth, kDdTabWidth);
}

/// 标签高 33、离标题栏上沿 7（标题栏高 40：字在上部 26 像素里居中，下面 7 像素是选中标签接到画面的那段）
const kDdTabHeight = 33.0;
const kDdTabTop = 7.0;

/// 按下到松开挪动不到这么多（逻辑像素）算点击，不算拖动。Flutter 对鼠标的拖动判定只要 2 像素，点的时候手一抖就成了拖动
const kDdClickSlop = 6.0;

/// 标题栏的拖动区：撑满给它的地方（按在上下空白里也能拖），按住拖窗口，双击最大化 / 还原。
/// 不用 onDoubleTap：它会和拖动抢鼠标（照 RustDesk 标签栏的做法自己判断两次点击的间隔）。里面的按钮、标签自己的点击照样优先
class DdTitleDragArea extends StatefulWidget {
  final Widget child;

  /// 主窗口（首页）：拖动、最大化走 windowManager（同 RustDesk 的 startDragging / toggleMaximize 的参数）
  final bool isMainWindow;
  const DdTitleDragArea({super.key, required this.child, this.isMainWindow = false});

  @override
  State<DdTitleDragArea> createState() => _DdTitleDragAreaState();
}

class _DdTitleDragAreaState extends State<DdTitleDragArea> {
  int _lastClickAt = 0;

  void _onTap() {
    final now = DateTime.now().millisecondsSinceEpoch;
    if (now - _lastClickAt < bind.getDoubleClickTime()) {
      _lastClickAt = 0;
      toggleMaximize(widget.isMainWindow).then((v) => stateGlobal.setMaximized(v));
    } else {
      _lastClickAt = now;
    }
  }

  @override
  Widget build(BuildContext context) => GestureDetector(
        behavior: HitTestBehavior.translucent,
        onTap: _onTap,
        onPanStart: (_) {
          // 拖过窗口再点一下不该凑成双击
          _lastClickAt = 0;
          startDragging(widget.isMainWindow);
        },
        child: SizedBox.expand(child: widget.child),
      );
}

/// 鼠标在不在上面：[builder] 收到当前的悬停状态。标题栏上的按钮、标签都只为一个悬停布尔值，不各写一个 State
class DdHover extends StatefulWidget {
  final MouseCursor cursor;
  final Widget Function(BuildContext context, bool hover) builder;
  const DdHover({super.key, required this.builder, this.cursor = MouseCursor.defer});

  @override
  State<DdHover> createState() => _DdHoverState();
}

class _DdHoverState extends State<DdHover> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) => MouseRegion(
        cursor: widget.cursor,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: widget.builder(context, _hover),
      );
}

/// 标题栏上的圆角小按钮（原型 dd-btn--small：高 28、左右 8、6 圆角），悬停底和标签悬停一样深。[tooltip] 为空就不带悬停提示
class DdBarButton extends StatelessWidget {
  final Widget child;
  final String? tooltip;
  final VoidCallback? onTap;
  const DdBarButton({super.key, required this.child, this.tooltip, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final button = DdHover(
      cursor: SystemMouseCursors.click,
      builder: (context, hover) => GestureDetector(
        onTap: onTap,
        child: Container(
          height: 28,
          margin: const EdgeInsets.symmetric(horizontal: 2),
          padding: const EdgeInsets.symmetric(horizontal: 8),
          decoration: BoxDecoration(
              color: hover ? DdColors.tabHover : Colors.transparent, borderRadius: BorderRadius.circular(DdRadius.md)),
          child: Center(child: child),
        ),
      ),
    );
    return tooltip == null ? button : Tooltip(message: tooltip!, child: button);
  }
}

/// 标签的外壳（形状、宽、边距）：选中的白底、上两角圆、下两角向外弯，没选中的悬停时一块深色圆角底；[padding] 是内容离壳的边距，
/// 底下要留 [kDdTabTop]（字在标签上部 26 像素里居中，和没选中的标签、标题栏里其它字对齐）。悬停状态由外面给（[DdHover]），
/// 点击、右键、拖动、窄时的悬停提示也各标签自己包：两种标签包的位置不同，提示出现的位置也就不同
class DdChromeTab extends StatelessWidget {
  final bool selected;
  final bool hover;
  final double width;
  final EdgeInsetsGeometry padding;
  final Widget child;
  const DdChromeTab(
      {super.key,
      required this.selected,
      required this.hover,
      required this.width,
      required this.padding,
      required this.child});

  @override
  Widget build(BuildContext context) {
    final bg = selected ? DdColors.surface : (hover ? DdColors.tabHover : Colors.transparent);
    return CustomPaint(
      painter: DdTabShape(color: bg, flare: selected),
      child: Container(
        width: width,
        height: kDdTabHeight,
        margin: const EdgeInsets.only(top: kDdTabTop),
        padding: padding,
        child: child,
      ),
    );
  }
}

/// 标签的底：选中的（[flare]）上两角圆角 8、下两角再向外弯出 8 像素，画到标签外面、和标题栏下沿接上（Chrome 的标签形状）；
/// 没选中的悬停时是一块四边各留 7 像素的 6 圆角长方形（照 UU；只剩图标时是正方形）。标签的子部件带着 [kDdTabTop] 的上边距，形状从那里画起
class DdTabShape extends CustomPainter {
  final Color color;
  final bool flare;
  const DdTabShape({required this.color, required this.flare});

  @override
  void paint(Canvas canvas, Size size) {
    if (color.alpha == 0) return;
    const r = 8.0;
    const top = kDdTabTop;
    final w = size.width, h = size.height;
    if (!flare) {
      canvas.drawRRect(
          RRect.fromLTRBR(top, top, w - top, h - top, const Radius.circular(DdRadius.md)), Paint()..color = color);
      return;
    }
    final path = Path()
      ..moveTo(-r, h)
      ..arcToPoint(Offset(0, h - r), radius: const Radius.circular(r), clockwise: false)
      ..lineTo(0, top + r)
      ..arcToPoint(Offset(r, top), radius: const Radius.circular(r))
      ..lineTo(w - r, top)
      ..arcToPoint(Offset(w, top + r), radius: const Radius.circular(r))
      ..lineTo(w, h - r)
      ..arcToPoint(Offset(w + r, h), radius: const Radius.circular(r), clockwise: false)
      ..close();
    canvas.drawPath(path, Paint()..color = color);
  }

  @override
  bool shouldRepaint(covariant DdTabShape oldDelegate) => oldDelegate.color != color || oldDelegate.flare != flare;
}

/// 1 像素细线图标对齐像素要挪的量：线宽是奇数个物理像素时线心落在两个像素中间，挪半个物理像素才不发虚；
/// 125% / 150% 这类非整数倍缩放怎么挪都对不齐，不挪
Offset ddHairlineNudge(BuildContext context) {
  final dpr = MediaQuery.devicePixelRatioOf(context);
  final n = dpr.round();
  if ((dpr - n).abs() > 0.01 || n.isEven) return Offset.zero;
  final d = 0.5 / n;
  return Offset(d, d);
}

/// 窗口按钮（照 UU / Windows 11：46 宽、占满标题栏高，关闭悬停红底白叉）。最小化、最大化、关闭做的事照 RustDesk 的
/// WindowActionPanel；那个面板只有画法和这三件事，没有别的监听，换掉它不丢行为。
/// 主窗口、远程窗口和文件传输 / 终端 / 端口映射 / 连接管理器这几个子窗口共用；[onClose] 同 WindowActionPanel 的（返回假就不关），
/// [isMainWindow] 同它的（主窗口走 windowManager，关闭是藏到托盘；主窗口的底浅，悬停用普通浅灰，远程 / 子窗口用 winHover）；
/// [height] 是所在标题栏的高
class DdWindowButtons extends StatelessWidget {
  final Future<bool> Function()? onClose;
  final bool isMainWindow;
  final bool showMinimize;
  final bool showMaximize;
  final bool showClose;
  final double height;
  const DdWindowButtons(
      {super.key,
      this.onClose,
      this.isMainWindow = false,
      this.showMinimize = true,
      this.showMaximize = true,
      this.showClose = true,
      this.height = DdSize.titlebarRemote});

  @override
  Widget build(BuildContext context) {
    if (isMacOS) return const SizedBox.shrink();
    return Row(mainAxisSize: MainAxisSize.min, children: [
      if (showMinimize)
        _WinButton(
          icon: DdIcon.winMinimize,
          tooltip: translate('Minimize'),
          height: height,
          isMainWindow: isMainWindow,
          onTap: () =>
              isMainWindow ? windowManager.minimize() : WindowController.fromWindowId(kWindowId!).minimize(),
        ),
      if (showMaximize)
        Obx(() => _WinButton(
              icon: stateGlobal.isMaximized.isTrue ? DdIcon.winRestore : DdIcon.winMaximize,
              tooltip: translate(stateGlobal.isMaximized.isTrue ? 'Restore' : 'Maximize'),
              height: height,
              isMainWindow: isMainWindow,
              // 子窗口最大化 / 还原不会触发窗口事件，自己更新状态（同 RustDesk）
              onTap: () => toggleMaximize(isMainWindow).then((v) => stateGlobal.setMaximized(v)),
            )),
      if (showClose)
        _WinButton(
          icon: DdIcon.winClose,
          tooltip: translate('Close'),
          height: height,
          isMainWindow: isMainWindow,
          isClose: true,
          onTap: () async {
            if (await onClose?.call() ?? true) {
              Future.delayed(Duration.zero, () async {
                if (isMainWindow) {
                  await windowManager.close();
                } else {
                  await WindowController.fromWindowId(kWindowId!).close();
                }
              });
            }
          },
        ),
    ]);
  }
}

class _WinButton extends StatelessWidget {
  final DdIcon icon;
  final String tooltip;
  final VoidCallback onTap;
  final bool isClose;
  final bool isMainWindow;
  final double height;
  const _WinButton(
      {required this.icon,
      required this.tooltip,
      required this.onTap,
      required this.height,
      required this.isMainWindow,
      this.isClose = false});

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      waitDuration: const Duration(milliseconds: 600),
      child: DdHover(
        builder: (context, hover) {
          final red = isClose && hover;
          return GestureDetector(
            onTap: onTap,
            child: Container(
              width: 46,
              height: height,
              color: red
                  ? DdColors.closeHover
                  : (hover ? (isMainWindow ? DdColors.hover : DdColors.winHover) : Colors.transparent),
              alignment: Alignment.center,
              // Lucide 的线在 16 像素下是 1 像素宽、正落在两行 / 两列像素中间，挪半个物理像素才不发虚（原型同样处理；
              // 挪多少按屏幕缩放算，见 ddHairlineNudge）；「还原」是 Fluent 16 号原图，本来就对齐像素，不挪
              child: Transform.translate(
                  offset: icon == DdIcon.winRestore ? Offset.zero : ddHairlineNudge(context),
                  child: ddIcon(icon, size: 16, color: red ? DdColors.onAccent : DdColors.ink)),
            ),
          );
        },
      ),
    );
  }
}

/// 机器名（远程窗口标题栏左上角、文件传输 / 终端 / 端口映射窗口的标题栏和标签共用）：受管版用户不认设备号，有备注名用备注名，
/// 否则用主机名（RustDesk 的标签是「设备号@主机名」），都没有（还没连上过）才用设备号。[pi] 是这次连接拿到的对端信息（有它的窗口给），
/// 主机名优先用它里面的（peer 配置里存的是上次的）；没给或还没拿到时退回配置里记的。
/// 在 Obx 里调用时备注名改了会跟着变
String ddPeerTitle(String id, {PeerInfo? pi}) {
  _aliasChanged(id).value;
  final alias = bind.mainGetPeerOptionSync(id: id, key: 'alias');
  if (alias.isNotEmpty) return alias;
  if (pi != null && pi.isSet.isTrue && pi.hostname.isNotEmpty) return pi.hostname;
  try {
    final hostname = jsonDecode(bind.mainGetPeerSync(id: id))['info']['hostname'];
    if (hostname is String && hostname.isNotEmpty) return hostname;
  } catch (e) {
    debugPrint('dd: peer hostname: $e');
  }
  return id;
}

/// 备注名改了时让画名字的 Obx 重画：RustDesk 改备注名后给这台机器的各个会话推 alias 事件（FfiModel.handleAliasChanged），
/// 更新的是 PeerStringOption 'tabLabel'；它自己从没初始化过这个值（事件来了找不到、抛错），这里第一次用时初始化。
/// 不能提前在某个部件的 initState 里一次初始化：窗口会复用给别的机器、子窗口的标签随时增减，机器号是画的时候才知道的
RxString _aliasChanged(String id) {
  if (!Get.isRegistered<RxString>(tag: PeerStringOption.tag(id, 'tabLabel'))) {
    PeerStringOption.init(
        id, 'tabLabel', () => getDesktopTabLabel(id, bind.mainGetPeerOptionSync(id: id, key: 'alias')));
  }
  return PeerStringOption.find(id, 'tabLabel');
}
