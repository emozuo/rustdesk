// DreamDesk 远程窗口的标题栏（dreamdesk/docs/M-受管版方案.md 第 3b 步，用户 10-26 定：照网易 UU 远程）：一台机器一个窗口，
// 标题栏左边机器名 + 连接安全标识，中间每块远程屏一个标签（点了切到那块屏），右边信号格（先占位）+ 连接时长、麦克风、「控制中心」、
// 窗口按钮。画面上的悬浮工具栏不要了，操作都收进控制中心（菜单项调用 RustDesk 原工具栏背后的同一个功能，挑的时候按它的动作编号）。
// 只在受管版用（远程窗口的标签组件 DesktopTab 收 `customBar` 时整条换成这个）；窗口关闭、最大化、记窗口位置等照旧由 DesktopTab 管。
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/shared_state.dart';
import 'package:flutter_hbb/common/widgets/toolbar.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/desktop/pages/remote_page.dart';
import 'package:flutter_hbb/desktop/widgets/dd_icons.dart';
import 'package:flutter_hbb/desktop/widgets/dd_theme.dart';
import 'package:flutter_hbb/desktop/widgets/remote_toolbar.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/models/chat_model.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:flutter_hbb/utils/dd_kiosk.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:get/get.dart';

// 和主窗口首页同一套 UU 的浅色
const _kChrome = DdColors.chrome;
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
                  Container(width: 1, height: 16, color: _kLine, margin: const EdgeInsets.symmetric(horizontal: 12)),
                  Flexible(child: _DisplayTabs(id: id, ffi: ffi)),
                ]),
              ),
            ),
            ddIcon(DdIcon.signal, size: 16, color: _kGreen),
            const SizedBox(width: 6),
            _Elapsed(key: ValueKey(id), ffi: ffi),
            _MicButton(ffi: ffi),
            const SizedBox(width: 4),
            _ControlCenter(id: id, ffi: ffi),
            Container(width: 1, height: 16, color: _kLine, margin: const EdgeInsets.only(left: 8, right: 4)),
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
      width: 60,
      child: Text(text,
          style: const TextStyle(fontSize: 12, color: _kText2, fontFeatures: [FontFeature.tabularFigures()])),
    );
  }
}

/// 机器名（别名优先，和 RustDesk 标签上的一样）+ 连接安全标识（加密 / 中继的小图标，悬停看说明）
class _PeerName extends StatelessWidget {
  final String id;
  const _PeerName({required this.id});

  @override
  Widget build(BuildContext context) {
    return Obx(() {
      final name = DesktopTab.tablabelGetter(id).value;
      final type = ConnectionTypeState.find(id);
      return Row(mainAxisSize: MainAxisSize.min, children: [
        Text(name.toUpperCase(), style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600, color: _kText)),
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
class _DisplayTabs extends StatelessWidget {
  final String id;
  final FFI ffi;
  const _DisplayTabs({required this.id, required this.ffi});

  @override
  Widget build(BuildContext context) {
    return Obx(() {
      final pi = ffi.ffiModel.pi;
      final count = pi.displays.length;
      final current = CurrentDisplayState.find(id).value;
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
          child: Container(
          height: 34,
          margin: const EdgeInsets.only(top: 6, right: 2),
          padding: const EdgeInsets.symmetric(horizontal: 14),
          decoration: BoxDecoration(
            color: bg,
            borderRadius: const BorderRadius.vertical(top: Radius.circular(8)),
          ),
          child: Row(mainAxisSize: MainAxisSize.min, children: [
            ddIcon(DdIcon.monitor, size: 15, color: _kText),
            const SizedBox(width: 6),
            Text(widget.label, style: const TextStyle(fontSize: 13, color: _kText)),
          ]),
          ),
        ),
      ),
    );
  }
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
    return MenuAnchor(
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
          ddIcon(DdIcon.controlCenter, size: 17),
          const SizedBox(width: 6),
          Text(translate('Control center'), style: const TextStyle(fontSize: 13, color: _kText)),
        ]),
      ),
    );
  }
}

/// 控制中心的菜单（标题栏的「控制中心」和全屏时的悬浮球共用一份）。[fullscreen]：在悬浮球上（全屏时），「全屏」换成「退出全屏」；
/// kiosk 始终全屏，不给这一项
List<Widget> ddControlMenu(BuildContext context, String id, FFI ffi, {bool fullscreen = false}) {
    final pi = ffi.ffiModel.pi;
    final count = pi.displays.length;
    final current = CurrentDisplayState.find(id).value;
    final controls = toolbarControls(context, id, ffi);
    Widget control(String actionId) {
      final m = controls.firstWhereOrNull((m) => m.actionId == actionId);
      return m == null ? const SizedBox.shrink() : MenuButton(onPressed: m.onPressed, ffi: ffi, child: m.child);
    }

    Widget toggles(Future<List<TToggleMenu>> future, List<String> actionIds) => FutureBuilder<List<TToggleMenu>>(
          future: future,
          builder: (_, snap) => Column(mainAxisSize: MainAxisSize.min, children: [
            for (final a in actionIds)
              if (snap.data?.firstWhereOrNull((m) => m.actionId == a) case final m?)
                CkbMenuButton(value: m.value, onChanged: m.onChanged, ffi: ffi, child: m.child),
          ]),
        );
    Widget radios(Future<List<TRadioMenu<String>>> future) => FutureBuilder<List<TRadioMenu<String>>>(
          future: future,
          builder: (_, snap) => Column(mainAxisSize: MainAxisSize.min, children: [
            for (final m in snap.data ?? const <TRadioMenu<String>>[])
              RdoMenuButton<String>(
                  value: m.value, groupValue: m.groupValue, onChanged: m.onChanged, ffi: ffi, child: m.child),
          ]),
        );
    Widget sub(DdIcon icon, String label, List<Widget> children) => SubmenuButton(
          menuStyle: ddMenuStyle,
          leadingIcon: ddIcon(icon, size: 16),
          menuChildren: children,
          child: Text(label),
        );
    Widget item(DdIcon icon, String label, VoidCallback? onPressed, {Color? color}) => MenuItemButton(
          leadingIcon: ddIcon(icon, size: 16, color: color ?? _kText),
          onPressed: onPressed,
          child: Text(label, style: TextStyle(color: color)),
        );

    return [
      sub(DdIcon.monitor, '${translate('Screen {${current + 1}}')} (${id.toUpperCase()})', [
        for (var i = 0; i < count; i++)
          RdoMenuButton<int>(
            value: i,
            groupValue: current,
            ffi: ffi,
            onChanged: (_) => openMonitorInTheSameTab(i, ffi, pi),
            child: Text('${translate('Screen {${i + 1}}')}  '
                '${pi.displays[i].width}×${pi.displays[i].height}'),
          ),
      ]),
      item(DdIcon.arrowLeftRight, translate('Switch to next screen'),
          count > 1 ? () => openMonitorInTheSameTab((current + 1) % count, ffi, pi) : null),
      const Divider(height: 9),
      sub(DdIcon.eye, translate('Image Quality'), [
        radios(toolbarImageQuality(context, id, ffi)),
        const Divider(height: 9),
        radios(toolbarCodec(context, id, ffi)),
        toggles(toolbarDisplayToggle(context, id, ffi), [kShortcutActionToggleTrueColor]),
      ]),
      sub(DdIcon.monitor, translate('Window'), [radios(toolbarViewStyle(context, id, ffi))]),
      sub(DdIcon.mic, translate('Sound'), [
        toggles(toolbarDisplayToggle(context, id, ffi), [kShortcutActionToggleMute]),
      ]),
      sub(DdIcon.info, translate('Security'), [
        control(kShortcutActionInsertLock),
        control(kShortcutActionToggleBlockInput),
        toggles(toolbarDisplayToggle(context, id, ffi), [kShortcutActionToggleLockAfterSessionEnd]),
      ]),
      sub(DdIcon.devices, translate('Peripherals'), [
        _mouseMode(ffi),
        const Divider(height: 9),
        toggles(toolbarDisplayToggle(context, id, ffi),
            [kShortcutActionToggleDisableClipboard, kShortcutActionToggleEnableFileCopyPaste]),
        // 反转滚轮、交换左右键：RustDesk 原样就是按机器存的（菜单项照它的工具栏）
        if (ffi.ffiModel.keyboard)
          CkbMenuButton(
            value: (bind.sessionGetReverseMouseWheelSync(sessionId: ffi.sessionId) ?? '') == 'Y' ||
                ((bind.sessionGetReverseMouseWheelSync(sessionId: ffi.sessionId) ?? '') == '' &&
                    bind.mainGetUserDefaultOption(key: kKeyReverseMouseWheel) == 'Y'),
            ffi: ffi,
            onChanged: ffi.ffiModel.viewOnly
                ? null
                : (v) {
                    if (v != null) bind.sessionSetReverseMouseWheel(sessionId: ffi.sessionId, value: v ? 'Y' : 'N');
                  },
            child: Text(translate('Reverse mouse wheel')),
          ),
        for (final m in toolbarKeyboardToggles(ffi))
          if (m.actionId == kShortcutActionToggleSwapLeftRightMouse)
            CkbMenuButton(value: m.value, onChanged: m.onChanged, ffi: ffi, child: m.child),
      ]),
      sub(DdIcon.terminal, translate('Tools'), [
        MenuButton(onPressed: () => connect(context, id, isFileTransfer: true), ffi: ffi, child: Text(translate('File transfer'))),
        MenuButton(onPressed: () => connect(context, id, isTerminal: true), ffi: ffi, child: Text(translate('Terminal'))),
        control(kShortcutActionToggleRecording),
        control(kShortcutActionScreenshot),
      ]),
      sub(DdIcon.menu, translate('More'), [
        control(kShortcutActionSendCtrlAltDel),
        control(kShortcutActionRestartRemote),
        control(kShortcutActionRefresh),
        toggles(toolbarDisplayToggle(context, id, ffi), [kShortcutActionToggleQualityMonitor]),
        MenuButton(
            onPressed: () => bind.sessionDdproRetry(sessionId: ffi.sessionId),
            ffi: ffi,
            child: Text(translate('Switch to pro mode'))),
      ]),
      const Divider(height: 9),
      if (!kDdKiosk)
        fullscreen
            ? item(DdIcon.arrowUpCircle, translate('Exit Fullscreen'), () => stateGlobal.setFullscreen(false))
            : item(DdIcon.arrowUpCircle, translate('Fullscreen'), () => stateGlobal.setFullscreen(true)),
      item(DdIcon.close, translate('End remote control'), () => closeConnection(id: id), color: DdColors.dangerText),
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
      final remote = snap.data ?? false;
      void choose(bool? v) {
        if (v == null) return;
        bind.sessionSetFlutterOption(sessionId: ffi.sessionId, k: _kDdMouseMode, v: v ? 'remote' : '');
        ffi.inputModel.setRelativeMouseMode(v);
      }

      return Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
        RdoMenuButton<bool>(value: false, groupValue: remote, ffi: ffi, onChanged: choose, child: Text(translate('Local mouse'))),
        RdoMenuButton<bool>(
            value: true,
            groupValue: remote,
            ffi: ffi,
            onChanged: reason.isEmpty ? choose : null,
            child: Text(translate('Remote mouse'))),
        if (reason.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 6),
            child: Text(reason, style: const TextStyle(fontSize: 12, color: _kText3)),
          ),
      ]);
    },
  );
}

/// 控制中心菜单的样式（白底圆角，照 UU），悬浮球的菜单也用它
final ddMenuStyle = MenuStyle(
  backgroundColor: const WidgetStatePropertyAll(Colors.white),
  surfaceTintColor: const WidgetStatePropertyAll(Colors.white),
  elevation: const WidgetStatePropertyAll(8),
  shape: WidgetStatePropertyAll(RoundedRectangleBorder(borderRadius: BorderRadius.circular(8))),
  padding: const WidgetStatePropertyAll(EdgeInsets.symmetric(vertical: 6)),
);
