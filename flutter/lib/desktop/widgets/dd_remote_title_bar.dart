// DreamDesk 远程窗口的标题栏（dreamdesk/docs/M-受管版方案.md 第 3b 步，用户 10-26 定：照网易 UU 远程）：一台机器一个窗口，
// 标题栏左边机器名 + 连接安全标识，中间每块远程屏一个标签（点了切到那块屏），右边信号格（先占位）+ 连接时长、麦克风、「控制中心」、
// 窗口按钮。画面上的悬浮工具栏不要了，操作都收进控制中心（菜单项调用 RustDesk 原工具栏背后的同一个功能，挑的时候按它的动作编号）。
// 只在受管版用（远程窗口的标签组件 DesktopTab 收 `customBar` 时整条换成这个）；窗口关闭、最大化、记窗口位置等照旧由 DesktopTab 管。
import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/shared_state.dart';
import 'package:flutter_hbb/common/widgets/toolbar.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/desktop/pages/remote_page.dart';
import 'package:flutter_hbb/desktop/widgets/dd_icons.dart';
import 'package:flutter_hbb/desktop/widgets/dd_theme.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/models/chat_model.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:flutter_hbb/utils/dd_kiosk.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:get/get.dart';

// 照 UU 的远程窗口：标题栏比主窗口深一档（10-27，原型 dreamdesk/design/prototype/Remote.html）
const _kChrome = DdColors.chromeRemote;
const _kTabSelected = DdColors.surface;
const _kHover = DdColors.hover;
const _kText = DdColors.ink;
const _kText2 = DdColors.ink2;
const _kText3 = DdColors.ink3;
const _kLine = DdColors.line;
const _kBlue = DdColors.accent;
const _kGreen = DdColors.online;

class DdRemoteTitleBar extends StatefulWidget {
  final DesktopTabController tabController;
  final Future<bool> Function() onClose;
  const DdRemoteTitleBar({Key? key, required this.tabController, required this.onClose}) : super(key: key);

  @override
  State<DdRemoteTitleBar> createState() => _DdRemoteTitleBarState();
}

class _DdRemoteTitleBarState extends State<DdRemoteTitleBar> {
  int _lastClickAt = 0;

  /// 双击标题栏空白处最大化 / 还原。不用 onDoubleTap：它会和拖动抢鼠标（照 RustDesk 标签栏的做法自己判断）
  void _onTap() {
    final now = DateTime.now().millisecondsSinceEpoch;
    if (now - _lastClickAt < bind.getDoubleClickTime()) {
      _lastClickAt = 0;
      toggleMaximize(false).then((v) => stateGlobal.setMaximized(v));
    } else {
      _lastClickAt = now;
    }
  }

  @override
  Widget build(BuildContext context) {
    return Theme(
      data: MyTheme.lightTheme.copyWith(splashFactory: NoSplash.splashFactory, highlightColor: Colors.transparent),
      child: Material(
        color: _kChrome,
        child: Obx(() {
          final tabs = widget.tabController.state.value.tabs;
          if (tabs.isEmpty || tabs.first.page is! RemotePage) return const SizedBox.shrink();
          final id = tabs.first.key;
          final ffi = (tabs.first.page as RemotePage).ffi;
          return Row(children: [
            Expanded(
              child: GestureDetector(
                behavior: HitTestBehavior.translucent,
                onTap: _onTap,
                onPanStart: (_) => startDragging(false),
                child: Row(children: [
                  const SizedBox(width: 14),
                  loadIcon(18),
                  const SizedBox(width: 8),
                  _PeerName(id: id),
                  Container(width: 1, height: 16, color: _kLine, margin: const EdgeInsets.only(left: 12)),
                  // 照 UU：信号格和连接时长紧跟在屏幕标签后面，右边只留麦克风、控制中心、窗口按钮。
                  // 标签多到放不下时它占大头、可以横着滚，剩下的空白留给拖动窗口
                  Flexible(flex: 20, child: _DisplayTabs(id: id, ffi: ffi)),
                  // 标签滚动区右边自带 8 像素（留给弯角），合起来离信号格 12
                  const SizedBox(width: 4),
                  ddIcon(DdIcon.signal, size: 16, color: _kGreen),
                  const SizedBox(width: 6),
                  _Elapsed(key: ValueKey(id), ffi: ffi),
                  const Spacer(),
                ]),
              ),
            ),
            _MicButton(ffi: ffi),
            const SizedBox(width: 4),
            _ControlCenter(id: id, ffi: ffi),
            const SizedBox(width: 8),
            WindowActionPanel(
              isMainWindow: false,
              state: widget.tabController.state,
              tabController: widget.tabController,
              invisibleTabKeys: RxList<String>.empty(),
              onClose: widget.onClose,
            ),
          ]);
        }),
      ),
    );
  }

}

/// 连接时长：从第一次拿到对端信息时算起（换了连的机器整个部件重建、重新算），每秒走一格。
/// 单独成一个部件、自己刷新：整条标题栏不用跟着每秒重建
class _Elapsed extends StatefulWidget {
  final FFI ffi;
  const _Elapsed({Key? key, required this.ffi}) : super(key: key);

  @override
  State<_Elapsed> createState() => _ElapsedState();
}

class _ElapsedState extends State<_Elapsed> {
  DateTime? _since;
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    if (widget.ffi.ffiModel.pi.isSet.isTrue) _since = DateTime.now();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (_since == null && widget.ffi.ffiModel.pi.isSet.isTrue) _since = DateTime.now();
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final since = _since;
    var text = '';
    if (since != null) {
      final s = DateTime.now().difference(since).inSeconds;
      String two(int v) => v.toString().padLeft(2, '0');
      text = '${two(s ~/ 3600)}:${two(s % 3600 ~/ 60)}:${two(s % 60)}';
    }
    return SizedBox(
      width: 64,
      child: Text(text,
          style: const TextStyle(fontSize: 13, color: _kText2, fontFeatures: [FontFeature.tabularFigures()])),
    );
  }
}

/// 左上角的名字（照原型只写名字）：受管版用户不认设备号，有备注名用备注名，否则用主机名（RustDesk 的标签是「设备号@主机名」），
/// 都没有（还没连上过）才用设备号
String _peerTitle(String id) {
  final alias = bind.mainGetPeerOptionSync(id: id, key: 'alias');
  if (alias.isNotEmpty) return alias;
  try {
    final hostname = jsonDecode(bind.mainGetPeerSync(id: id))['info']['hostname'];
    if (hostname is String && hostname.isNotEmpty) return hostname;
  } catch (e) {
    debugPrint('dd: peer hostname: $e');
  }
  return id;
}

/// 机器名（[_peerTitle]）+ 连接安全标识（加密 / 中继的小图标，悬停看说明）
class _PeerName extends StatelessWidget {
  final String id;
  const _PeerName({required this.id});

  @override
  Widget build(BuildContext context) {
    return Obx(() {
      final name = _peerTitle(id);
      final type = ConnectionTypeState.find(id);
      return Row(mainAxisSize: MainAxisSize.min, children: [
        Text(name.toUpperCase(), style: const TextStyle(fontSize: 13, color: _kText)),
        if (type.isValid()) ...[
          const SizedBox(width: 6),
          Tooltip(
            message: getConnectionText(type.secure.value == ConnectionType.strSecure,
                type.direct.value == ConnectionType.strDirect, type.stream_type.value),
            child: SvgPicture.asset('assets/${type.secure.value}${type.direct.value}.svg', width: 14, height: 14),
          ),
        ],
      ]);
    });
  }
}

/// 每块远程屏一个标签，选中的白底圆角上沿（照 UU / 浏览器标签）；点了切到那块屏（同一条连接、同一个窗口）
/// 这个窗口在看的屏；-1 表示不在看某一块（RustDesk 的「所有屏」是 -1，屏变少后记着的号也可能越界），这时标签都不选中、
/// 控制中心不写屏号（照 RustDesk 切屏按钮的范围判断）
int _currentDisplay(String id, int count) {
  final d = CurrentDisplayState.find(id).value;
  return d >= 0 && d < count ? d : -1;
}

class _DisplayTabs extends StatelessWidget {
  final String id;
  final FFI ffi;
  const _DisplayTabs({required this.id, required this.ffi});

  @override
  Widget build(BuildContext context) {
    return Obx(() {
      final pi = ffi.ffiModel.pi;
      final count = pi.displays.length;
      final current = _currentDisplay(id, count);
      if (count == 0) return const SizedBox.shrink();
      // 把一块屏拖出去（或右键「在新窗口打开」）：用 RustDesk 的「显示器开到新窗口」，同一条连接、不另连。
      // 拖的是正在看的这块就先让这个窗口换去看下一块，免得两个窗口看同一块；只有一块屏时不能拖出
      void openOut(int i) {
        if (count < 2) return;
        if (i == current) openMonitorInTheSameTab((i + 1) % count, ffi, pi);
        openMonitorInNewTabOrWindow(i, id, pi);
      }

      return SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        // 选中标签两个下角各向外弯出 8 像素（画在标签外面），两边留出这 8 像素，不然第一个、最后一个标签的弯角被滚动区裁掉
        padding: const EdgeInsets.symmetric(horizontal: 8),
        child: Row(crossAxisAlignment: CrossAxisAlignment.end, children: [
          for (var i = 0; i < count; i++)
            _DisplayTab(
              label: translate('Screen {${i + 1}}'),
              selected: i == current,
              onTap: i == current ? null : () => openMonitorInTheSameTab(i, ffi, pi),
              onOpenOut: count < 2 ? null : () => openOut(i),
            ),
        ]),
      );
    });
  }
}

class _DisplayTab extends StatefulWidget {
  final String label;
  final bool selected;
  final VoidCallback? onTap;
  /// 拖出标题栏松手、或右键菜单「在新窗口打开」时调用；null 表示不能拖出（只有一块屏）
  final VoidCallback? onOpenOut;
  const _DisplayTab({required this.label, required this.selected, this.onTap, this.onOpenOut});

  @override
  State<_DisplayTab> createState() => _DisplayTabState();
}

class _DisplayTabState extends State<_DisplayTab> {
  bool _hover = false;
  /// 拖着时标签跟着鼠标走的位移；往下拖出标题栏（超过这么多）松手就开新窗口
  Offset _drag = Offset.zero;
  static const _dragOut = 24.0;

  void _menu(TapDownDetails d) {
    final open = widget.onOpenOut;
    if (open == null) return;
    final overlay = Overlay.of(context).context.findRenderObject() as RenderBox;
    showMenu<int>(
      context: context,
      position: RelativeRect.fromRect(d.globalPosition & const Size(1, 1), Offset.zero & overlay.size),
      items: [PopupMenuItem<int>(value: 1, height: 36, child: Text(translate('Open in new window')))],
    ).then((v) {
      if (v == 1) open();
    });
  }

  @override
  Widget build(BuildContext context) {
    final bg = widget.selected ? _kTabSelected : (_hover ? _kHover : Colors.transparent);
    final canDrag = widget.onOpenOut != null;
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        onSecondaryTapDown: _menu,
        onPanUpdate: canDrag ? (d) => setState(() => _drag += d.delta) : null,
        onPanEnd: canDrag
            ? (_) {
                final out = _drag.dy > _dragOut;
                setState(() => _drag = Offset.zero);
                if (out) widget.onOpenOut!();
              }
            : null,
        onPanCancel: canDrag ? () => setState(() => _drag = Offset.zero) : null,
        child: Transform.translate(
          offset: _drag,
          // 照 UU（10-27 截图）：固定宽、内容靠左；选中的白底，上两角圆、下两角向外弯，和下面的画面连成一片（Chrome 的标签形状）
          child: CustomPaint(
            painter: _TabShape(color: bg, flare: widget.selected),
            child: Container(
              width: 230,
              height: 33,
              margin: const EdgeInsets.only(top: 7),
              padding: const EdgeInsets.symmetric(horizontal: 14),
              child: Row(children: [
                ddIcon(DdIcon.monitor, size: 16, color: _kText),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(widget.label,
                      overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 13, color: _kText)),
                ),
              ]),
            ),
          ),
        ),
      ),
    );
  }
}

/// 屏幕标签的底：上两角圆角 8；[flare] 时（选中的那个）下两角再向外弯出 8 像素，画到标签外面、和标题栏下沿接上。
/// 标签的子部件带着 7 像素的上边距，形状从那里画起
class _TabShape extends CustomPainter {
  final Color color;
  final bool flare;
  const _TabShape({required this.color, required this.flare});

  @override
  void paint(Canvas canvas, Size size) {
    if (color.alpha == 0) return;
    const r = 8.0;
    const top = 7.0;
    final w = size.width, h = size.height;
    final path = Path();
    if (flare) {
      path.moveTo(-r, h);
      path.arcToPoint(Offset(0, h - r), radius: const Radius.circular(r), clockwise: false);
    } else {
      path.moveTo(0, h);
    }
    path.lineTo(0, top + r);
    path.arcToPoint(Offset(r, top), radius: const Radius.circular(r));
    path.lineTo(w - r, top);
    path.arcToPoint(Offset(w, top + r), radius: const Radius.circular(r));
    if (flare) {
      path.lineTo(w, h - r);
      path.arcToPoint(Offset(w + r, h), radius: const Radius.circular(r), clockwise: false);
    } else {
      path.lineTo(w, h);
    }
    path.close();
    canvas.drawPath(path, Paint()..color = color);
  }

  @override
  bool shouldRepaint(covariant _TabShape old) => old.color != color || old.flare != flare;
}

/// 麦克风：开了就用 RustDesk 的语音通话把本机话筒送过去（专业模式悬浮球的「麦克风」也是它）
class _MicButton extends StatelessWidget {
  final FFI ffi;
  const _MicButton({required this.ffi});

  @override
  Widget build(BuildContext context) {
    return Obx(() {
      final status = ffi.chatModel.voiceCallStatus.value;
      final on = status == VoiceCallStatus.connected;
      final waiting = status == VoiceCallStatus.waitingForResponse;
      return _BarButton(
        tooltip: translate(on ? 'Turn off the microphone' : 'Turn on the microphone'),
        onTap: () => status == VoiceCallStatus.notStarted
            ? bind.sessionRequestVoiceCall(sessionId: ffi.sessionId)
            : bind.sessionCloseVoiceCall(sessionId: ffi.sessionId),
        child: ddIcon(on || waiting ? DdIcon.mic : DdIcon.micOff, size: 17, color: on ? _kBlue : (waiting ? _kText3 : _kText)),
      );
    });
  }
}

class _BarButton extends StatefulWidget {
  final Widget child;
  final String tooltip;
  final VoidCallback onTap;
  const _BarButton({required this.child, required this.tooltip, required this.onTap});

  @override
  State<_BarButton> createState() => _BarButtonState();
}

class _BarButtonState extends State<_BarButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: widget.tooltip,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Container(
            height: 28,
            padding: const EdgeInsets.symmetric(horizontal: 8),
            decoration: BoxDecoration(color: _hover ? _kHover : Colors.transparent, borderRadius: BorderRadius.circular(6)),
            child: Center(child: widget.child),
          ),
        ),
      ),
    );
  }
}

/// 「控制中心」：一级菜单照 UU（显示屏、切换下一个屏幕、画质、窗口、声音、安全、外设、工具、更多、全屏、退出远控）。
/// 菜单项挑用 RustDesk 原工具栏的同一批（按动作编号），行为、确认框都照原样；具体放哪些等菜单 / 设置方案定稿再调。
/// 不做快捷键（用户定），所以菜单项后面不标快捷键
class _ControlCenter extends StatefulWidget {
  final String id;
  final FFI ffi;
  const _ControlCenter({required this.id, required this.ffi});

  @override
  State<_ControlCenter> createState() => _ControlCenterState();
}

class _ControlCenterState extends State<_ControlCenter> {
  final _controller = MenuController();
  // 点开时才生成菜单内容、开着期间不变：RustDesk 生成这些菜单项时会顺带登记动作、取当前值，没必要每次重建标题栏都做一遍
  List<Widget> _children = const [];

  @override
  Widget build(BuildContext context) {
    return ddMenuTheme(
      context,
      child: MenuAnchor(
        controller: _controller,
        alignmentOffset: const Offset(0, 6),
        style: ddMenuStyle,
        menuChildren: _children,
        builder: (context, controller, _) => _BarButton(
          tooltip: '',
          onTap: () {
            if (controller.isOpen) return controller.close();
            setState(() => _children = ddControlMenu(context, widget.id, widget.ffi));
            controller.open();
          },
          child: Row(mainAxisSize: MainAxisSize.min, children: [
            ddIcon(DdIcon.controlCenter, size: 20),
            const SizedBox(width: 6),
            Text(translate('Control center'), style: const TextStyle(fontSize: 13, color: _kText)),
          ]),
        ),
      ),
    );
  }
}

/// 控制中心的菜单（标题栏的「控制中心」和全屏时的悬浮球共用一份）。[fullscreen]：在悬浮球上（全屏时），「全屏」换成「退出全屏」；
/// kiosk 始终全屏，不给这一项
List<Widget> ddControlMenu(BuildContext context, String id, FFI ffi, {bool fullscreen = false}) {
    final pi = ffi.ffiModel.pi;
    final count = pi.displays.length;
    final current = _currentDisplay(id, count);
    final controls = toolbarControls(context, id, ffi);
    // RustDesk 工具栏这次给了的那几项（没权限、对端不支持的它不给），按 (动作编号, 图标) 的顺序
    List<Widget> actions(List<(String, DdIcon)> wanted) => [
          for (final (actionId, icon) in wanted)
            if (controls.firstWhereOrNull((m) => m.actionId == actionId) case final m?)
              _act(ffi, icon: icon, onPressed: m.onPressed, child: m.child),
        ];
    final security = actions([(kShortcutActionInsertLock, DdIcon.lock), (kShortcutActionToggleBlockInput, DdIcon.ban)]);
    final more = actions([
      (kShortcutActionSendCtrlAltDel, DdIcon.keyboard),
      (kShortcutActionRefresh, DdIcon.refresh),
      (kShortcutActionRestartRemote, DdIcon.power),
    ]);

    // [divider]：这一组前面的分隔线，组里一项都没有时不画（免得剩一条孤零零的线）
    Widget toggles(Future<List<TToggleMenu>> future, List<String> actionIds, {bool divider = false}) =>
        FutureBuilder<List<TToggleMenu>>(
          future: future,
          builder: (_, snap) {
            final items = [
              for (final a in actionIds)
                if (snap.data?.firstWhereOrNull((m) => m.actionId == a) case final m?)
                  // 剪贴板照原型写成正向的「允许同步剪贴板」：选中 = RustDesk 的「禁用剪贴板」没开，点了调同一个回调
                  if (a == kShortcutActionToggleDisableClipboard)
                    _pick(ffi,
                        on: !m.value,
                        onPressed: m.onChanged == null ? null : () => m.onChanged!(!m.value),
                        child: Text(translate('Enable clipboard')))
                  else
                    _pick(ffi, on: m.value, onPressed: m.onChanged == null ? null : () => m.onChanged!(!m.value), child: m.child),
            ];
            return Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              if (divider && items.isNotEmpty) const Divider(height: 9),
              ...items,
            ]);
          },
        );
    // [head]：这一组的小标题（照原型「画质」「编解码」），组里一项都没有时不写
    Widget radios(Future<List<TRadioMenu<String>>> future, {String? head, bool divider = false}) =>
        FutureBuilder<List<TRadioMenu<String>>>(
          future: future,
          builder: (_, snap) {
            final list = snap.data ?? const <TRadioMenu<String>>[];
            return Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              if (list.isNotEmpty) ...[
                if (divider) const Divider(height: 9),
                if (head != null) _head(head),
              ],
              for (final m in list)
                _pick(ffi,
                    on: m.value == m.groupValue,
                    onPressed: m.onChanged == null ? null : () => m.onChanged!(m.value),
                    child: m.child),
            ]);
          },
        );
    Widget sub(DdIcon icon, String label, List<Widget> children) => SubmenuButton(
          menuStyle: ddMenuStyle,
          leadingIcon: ddIcon(icon),
          menuChildren: [_kMenuMinWidth, ...children],
          child: Text(label),
        );
    Widget item(DdIcon icon, String label, VoidCallback? onPressed, {Color? color}) => MenuItemButton(
          leadingIcon: ddIcon(icon, color: color ?? _kText),
          onPressed: onPressed,
          child: Text(label, style: TextStyle(color: color)),
        );

    return [
      _kMenuMinWidth,
      sub(DdIcon.monitor, current < 0 ? translate('Monitor') : translate('Screen {${current + 1}}'), [
        for (var i = 0; i < count; i++)
          _pick(ffi,
            on: i == current,
            onPressed: () => openMonitorInTheSameTab(i, ffi, pi),
            extra: Text('${pi.displays[i].width}×${pi.displays[i].height}',
                style: const TextStyle(fontSize: 12, color: _kText2)),
            child: Text(translate('Screen {${i + 1}}')),
          ),
      ]),
      item(DdIcon.arrowLeftRight, translate('Switch to next screen'),
          count > 1 ? () => openMonitorInTheSameTab((current + 1) % count, ffi, pi) : null),
      const Divider(height: 9),
      sub(DdIcon.eye, translate('Image Quality'), [
        radios(toolbarImageQuality(context, id, ffi), head: translate('Image Quality')),
        radios(toolbarCodec(context, id, ffi), head: translate('Codec'), divider: true),
        toggles(toolbarDisplayToggle(context, id, ffi), [kShortcutActionToggleTrueColor], divider: true),
      ]),
      sub(DdIcon.panelTop, translate('Window'), [radios(toolbarViewStyle(context, id, ffi))]),
      sub(DdIcon.volume, translate('Sound'), [
        toggles(toolbarDisplayToggle(context, id, ffi), [kShortcutActionToggleMute]),
      ]),
      sub(DdIcon.lock, translate('Security'), [
        ...security,
        toggles(toolbarDisplayToggle(context, id, ffi), [kShortcutActionToggleLockAfterSessionEnd],
            divider: security.isNotEmpty),
      ]),
      sub(DdIcon.mouse, translate('Peripherals'), [
        _mouseMode(ffi),
        const Divider(height: 9),
        toggles(toolbarDisplayToggle(context, id, ffi),
            [kShortcutActionToggleDisableClipboard, kShortcutActionToggleEnableFileCopyPaste]),
        // 反转滚轮、交换左右键：RustDesk 原样就是按机器存的（菜单项照它的工具栏）
        if (ffi.ffiModel.keyboard) _reverseWheel(ffi),
        for (final m in toolbarKeyboardToggles(ffi))
          if (m.actionId == kShortcutActionToggleSwapLeftRightMouse)
            _pick(ffi, on: m.value, onPressed: m.onChanged == null ? null : () => m.onChanged!(!m.value), child: m.child),
      ]),
      sub(DdIcon.wrench, translate('Tools'), [
        _act(ffi,
            icon: DdIcon.folderUp,
            onPressed: () => connect(context, id, isFileTransfer: true),
            child: Text(translate('File transfer'))),
        _act(ffi,
            icon: DdIcon.terminal,
            onPressed: () => connect(context, id, isTerminal: true),
            child: Text(translate('Terminal'))),
        // 录屏：桌面端 RustDesk 不放在工具栏菜单里（toolbarControls 只在手机上给），是单独的按钮 _RecordMenu；条件和动作照它
        if (bind.mainGetLocalOption(key: kOptionHideRecordingButton) != 'Y' &&
            (ffi.recordingModel.start || ffi.ffiModel.permissions['recording'] != false))
          _act(ffi,
              icon: DdIcon.record,
              onPressed: () => ffi.recordingModel.toggle(),
              child: Text(translate(ffi.recordingModel.start ? 'Stop session recording' : 'Start session recording'))),
        ...actions([(kShortcutActionScreenshot, DdIcon.camera)]),
      ]),
      sub(DdIcon.more, translate('More'), [
        ...more,
        toggles(toolbarDisplayToggle(context, id, ffi), [kShortcutActionToggleQualityMonitor], divider: more.isNotEmpty),
        const Divider(height: 9),
        _act(ffi,
            icon: DdIcon.zap,
            onPressed: () => bind.sessionDdproRetry(sessionId: ffi.sessionId),
            child: Text(translate('Switch to pro mode'))),
      ]),
      const Divider(height: 9),
      if (!kDdKiosk)
        fullscreen
            ? item(DdIcon.minimize, translate('Exit Fullscreen'), () => stateGlobal.setFullscreen(false))
            : item(DdIcon.maximize, translate('Fullscreen'), () => stateGlobal.setFullscreen(true)),
      item(DdIcon.logOut, translate('End remote control'), () => closeConnection(id: id), color: DdColors.dangerText),
    ];
}

/// 鼠标模式按机器记（dreamdesk/docs/M-菜单设置方案.md）：本地鼠标 = RustDesk 原来的绝对坐标；远程鼠标 = RustDesk 的相对鼠标模式
/// （本机指针锁在窗口里藏起来，只发移动量）。记在这台机器的配置里，每次连上（含重连）按记住的值打开（[ddApplyRememberedMouseMode]）。
/// RustDesk 自己在出错、重连、键盘权限被收回时会关掉相对模式，Ctrl+Alt 也能临时退出（防鼠标被锁死）——这些都不改记住的值，
/// 只有在菜单里切换才改
const _kDdMouseMode = 'dd-mouse-mode';

/// 不能用远程鼠标的原因；空串表示能用
String _remoteMouseUnavailable(FFI ffi) {
  if (!ffi.inputModel.isRelativeMouseModeSupported) return translate('The remote side is too old for the remote mouse');
  if (isLinux && bind.mainCurrentIsWayland()) return translate('The remote mouse does not work on a Wayland desktop');
  if (!ffi.ffiModel.keyboard || ffi.ffiModel.viewOnly) return translate('No keyboard and mouse permission');
  return '';
}

Future<bool> _remoteMouseRemembered(FFI ffi) async =>
    (await bind.sessionGetFlutterOption(sessionId: ffi.sessionId, k: _kDdMouseMode)) == 'remote';

/// 连上（含重连）拿到第一帧画面时调用：记住的是远程鼠标、又能用，就打开
Future<void> ddApplyRememberedMouseMode(FFI ffi) async {
  if (_remoteMouseUnavailable(ffi).isNotEmpty || !await _remoteMouseRemembered(ffi)) return;
  ffi.inputModel.setRelativeMouseMode(true);
}

Widget _mouseMode(FFI ffi) {
  final reason = _remoteMouseUnavailable(ffi);
  return FutureBuilder<bool>(
    future: _remoteMouseRemembered(ffi),
    builder: (_, snap) {
      // 记住的是远程鼠标、这次却用不了（没权限、Wayland、对端太旧）时连上就没打开，实际是本地鼠标，按实际的标
      final remote = (snap.data ?? false) && reason.isEmpty;
      void choose(bool? v) {
        if (v == null) return;
        // 开不成（画面尺寸还没量好、对端不支持；RustDesk 自己会提示）就不记，免得以后每次连上都再试、再提示
        if (!ffi.inputModel.setRelativeMouseMode(v) && v) return;
        bind.sessionSetFlutterOption(sessionId: ffi.sessionId, k: _kDdMouseMode, v: v ? 'remote' : '');
      }

      return Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        _head(translate('Mouse mode')),
        _pick(ffi, on: !remote, onPressed: () => choose(false), child: Text(translate('Local mouse'))),
        _pick(ffi, on: remote, onPressed: reason.isEmpty ? () => choose(true) : null, child: Text(translate('Remote mouse'))),
        // 能用时说明什么时候用、怎么退出；用不了时写原因（照原型）。外面的 width: 0 让这段说明不把菜单撑宽：
        // 量宽度时它算 0，排版时被上面的 stretch 拉到菜单宽、按菜单宽折行
        SizedBox(
          width: 0,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(10, 0, 10, 6),
            child: Text(
                reason.isNotEmpty
                    ? reason
                    // 退出远程鼠标的快捷键照 RustDesk（relative_mouse_model.dart）：macOS 是 Cmd+G，其它是 Ctrl+Alt
                    : translate(
                        'Use the remote mouse to turn the view in games and 3D software; press {${isMacOS ? 'Cmd+G' : 'Ctrl+Alt'}} to exit'),
                style: const TextStyle(fontSize: 12, height: 1.5, color: _kText2)),
          ),
        ),
      ]);
    },
  );
}

/// 控制中心里可以选中的一项（照 UU，不用复选框、单选框）：选中时文字主色、右边一个对勾；没选中的右边留同样宽的空，文字不跳。
/// 点了先刷新鼠标位置（照 RustDesk 的复选 / 单选菜单项），再调用它背后的同一个回调（复选：传相反的值；单选：传这一项的值）。
/// [extra]：对勾左边的灰字（如显示屏的分辨率）
Widget _pick(FFI ffi, {required bool on, required VoidCallback? onPressed, required Widget child, Widget? extra}) {
  final check = on ? ddIcon(DdIcon.check, size: 16, color: _kBlue) : const SizedBox(width: 16);
  return MenuItemButton(
    onPressed: onPressed == null
        ? null
        : () {
            ffi.inputModel.refreshMousePos();
            onPressed();
          },
    trailingIcon: extra == null ? check : Row(mainAxisSize: MainAxisSize.min, children: [extra, const SizedBox(width: 8), check]),
    child: on ? DefaultTextStyle.merge(style: const TextStyle(color: _kBlue), child: child) : child,
  );
}

/// 控制中心里的一个动作项，左边带图标；点了先让输入模型刷新鼠标位置（照 RustDesk 工具栏的 MenuButton）
Widget _act(FFI ffi, {required DdIcon icon, required VoidCallback? onPressed, required Widget child}) => MenuItemButton(
      leadingIcon: ddIcon(icon),
      onPressed: onPressed == null
          ? null
          : () {
              ffi.inputModel.refreshMousePos();
              onPressed();
            },
      child: child,
    );

/// 子菜单里一组的小标题（照原型：12 号灰字，和菜单项文字左对齐）
Widget _head(String text) => Padding(
      padding: const EdgeInsets.fromLTRB(10, 6, 10, 4),
      child: Text(text, style: const TextStyle(fontSize: 12, color: _kText2)),
    );

/// 反转滚轮：RustDesk 原样按机器存，没存过时跟用户默认值（照它工具栏的同一项）
Widget _reverseWheel(FFI ffi) {
  final v = bind.sessionGetReverseMouseWheelSync(sessionId: ffi.sessionId) ?? '';
  final on = v == 'Y' || (v == '' && bind.mainGetUserDefaultOption(key: kKeyReverseMouseWheel) == 'Y');
  return _pick(ffi,
    on: on,
    onPressed: ffi.ffiModel.viewOnly
        ? null
        : () => bind.sessionSetReverseMouseWheel(sessionId: ffi.sessionId, value: on ? 'N' : 'Y'),
    child: Text(translate('Reverse mouse wheel')),
  );
}

/// 控制中心菜单的样式（白底圆角，照 UU），悬浮球的菜单也用它
final ddMenuStyle = MenuStyle(
  backgroundColor: const WidgetStatePropertyAll(Colors.white),
  surfaceTintColor: const WidgetStatePropertyAll(Colors.white),
  elevation: const WidgetStatePropertyAll(8),
  shape: WidgetStatePropertyAll(RoundedRectangleBorder(borderRadius: BorderRadius.circular(8))),
  padding: const WidgetStatePropertyAll(EdgeInsets.all(6)),
);

/// 菜单最少 230 宽（照原型）。MenuStyle 的 minimumSize 不管用：Flutter 3.24 的菜单面板外面包了一层只约束竖向的
/// UnconstrainedBox，子菜单还没法关掉它；所以每个菜单开头放一个这么宽、高 0 的占位，菜单按最宽的一项定宽
const _kMenuMinWidth = SizedBox(width: 230 - 12);

/// 控制中心菜单项的样式（照原型 RemoteMenu：行高 36、圆角悬停块、字 14），套在菜单的 MenuAnchor 外面，子菜单也跟着用。
/// 子菜单右边的箭头是 Flutter 写死的 Material 实心三角，换不成原型的细箭头，只能调成次要文字的灰色
Widget ddMenuTheme(BuildContext context, {required Widget child}) => Theme(
      data: Theme.of(context).copyWith(
        menuButtonTheme: MenuButtonThemeData(
          style: ButtonStyle(
            visualDensity: VisualDensity.standard,
            minimumSize: const WidgetStatePropertyAll(Size(0, 36)),
            padding: const WidgetStatePropertyAll(EdgeInsets.symmetric(horizontal: 10)),
            textStyle: const WidgetStatePropertyAll(TextStyle(fontSize: 14, fontWeight: FontWeight.normal)),
            iconColor: const WidgetStatePropertyAll(_kText2),
            overlayColor: const WidgetStatePropertyAll(DdColors.hover),
            shape: WidgetStatePropertyAll(RoundedRectangleBorder(borderRadius: BorderRadius.circular(6))),
          ),
        ),
      ),
      child: child,
    );
