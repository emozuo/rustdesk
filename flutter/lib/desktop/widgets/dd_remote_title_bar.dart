// DreamDesk 远程窗口的标题栏（dreamdesk/docs/M-受管版方案.md 第 3b 步，用户 10-26 定：照网易 UU 远程）：一台机器一个窗口，
// 标题栏左边机器名 + 连接安全标识，中间每块远程屏一个标签（点了切到那块屏），右边信号格（先占位）+ 连接时长、麦克风、「控制中心」、
// 窗口按钮。画面上的悬浮工具栏不要了，操作都收进控制中心（dd_control_menu.dart）。
// 只在受管版用（远程窗口的标签组件 DesktopTab 收 `customBar` 时整条换成这个）；窗口按钮（dd_titlebar_common.dart）自己画
// （做的事照 RustDesk 的 WindowActionPanel），记窗口位置等照旧由 DesktopTab 管。
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/shared_state.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/desktop/pages/remote_page.dart';
import 'package:flutter_hbb/desktop/widgets/dd_control_menu.dart';
import 'package:flutter_hbb/desktop/widgets/dd_icons.dart';
import 'package:flutter_hbb/desktop/widgets/dd_remote_displays.dart';
import 'package:flutter_hbb/desktop/widgets/dd_remote_stats.dart';
import 'package:flutter_hbb/desktop/widgets/dd_theme.dart';
import 'package:flutter_hbb/desktop/widgets/dd_titlebar_common.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/models/chat_model.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:get/get.dart';


// 照 UU 的远程窗口：标题栏比主窗口深一档（10-27，原型 dreamdesk/design/prototype/Remote.html）
const _kChrome = DdColors.chromeRemote;
const _kText = DdColors.ink;
const _kText2 = DdColors.ink2;
const _kText3 = DdColors.ink3;
const _kBlue = DdColors.accent;
const _kTabText = DdColors.titleText;
const _kSep = DdColors.titleSep;

/// 主题用全局的受管版主题（ddLightTheme），不在这里另套：另套一层的话，控制中心菜单、悬停提示、右键菜单都跟着变，
/// 和全屏时悬浮球里的同一份菜单、和别的窗口的提示长得不一样
class DdRemoteTitleBar extends StatelessWidget {
  final DesktopTabController tabController;
  final Future<bool> Function() onClose;
  const DdRemoteTitleBar({Key? key, required this.tabController, required this.onClose}) : super(key: key);

  @override
  Widget build(BuildContext context) {
    return Material(
      color: _kChrome,
      // 鼠标进标题栏时重量一次本机显示器数（「铺满我所有显示器」按钮要两块以上才出现，开着窗口也可能插拔显示器）
      child: MouseRegion(
        onEnter: (_) => ddRefreshLocalScreens(),
        child: Obx(() {
          final tabs = tabController.state.value.tabs;
          if (tabs.isEmpty || tabs.first.page is! RemotePage) return const SizedBox.shrink();
          final id = tabs.first.key;
          final page = tabs.first.page as RemotePage;
          final ffi = page.ffi;
          return Row(children: [
            Expanded(
              // 麦克风、控制中心也放在拖动区里：它们只有 28 高，按在上下的空白里也要能拖窗口
              child: DdTitleDragArea(
                child: Row(children: [
                  // 左边这一段（机器名、标签、信号格、时长）占满麦克风左边的地方，没用完的留在它右边当拖动的空白；
                  // 麦克风、控制中心在它外面，始终贴着窗口按钮
                  Expanded(
                    child: Row(children: [
                      const SizedBox(width: 14),
                      loadIcon(18),
                      const SizedBox(width: 8),
                      _PeerName(id: id, ffi: ffi),
                      // 离第一个标签 12（标签滚动区左边自带 8 像素，留给弯角）
                      const SizedBox(width: 4),
                      // 照 UU：信号格和连接时长紧跟在屏幕标签后面，右边只留麦克风、控制中心、窗口按钮。
                      // 标签按能用的宽渐进变窄（_DisplayTabs），放不下最窄的才横着滚
                      Flexible(child: _DisplayTabs(page: page)),
                      _FillMyDisplays(page: page),
                      // 竖线只在标签和信号格之间（标签滚动区右边自带 8 像素，合起来离竖线 12）
                      Container(width: 1, height: 16, color: _kSep, margin: const EdgeInsets.only(left: 4, right: 12)),
                      DdSignal(key: ValueKey(id), id: id, ffi: ffi),
                      const SizedBox(width: 6),
                      _Elapsed(key: ValueKey(id), ffi: ffi),
                      // 再窄也至少留这么宽给拖动窗口（原型同样留 16）
                      const SizedBox(width: 16),
                    ]),
                  ),
                  _MicButton(ffi: ffi),
                  _ControlCenter(id: id, ffi: ffi),
                ]),
              ),
            ),
            DdWindowButtons(onClose: onClose),
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
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
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
    // 和质量监测面板用同一个起点（dd_remote_stats.dart）
    final text = ddElapsedText(widget.ffi);
    return SizedBox(
      width: 64,
      child: Text(text,
          style: const TextStyle(fontSize: 12, color: _kText2, fontFeatures: [FontFeature.tabularFigures()])),
    );
  }
}

/// 机器名 + 连接安全标识（加密 / 中继的小图标，悬停看说明）。名字跟着变：改备注名、连上后拿到主机名都会重画
class _PeerName extends StatelessWidget {
  final String id;
  final FFI ffi;
  const _PeerName({required this.id, required this.ffi});

  @override
  Widget build(BuildContext context) {
    return Obx(() {
      final name = ddPeerTitle(id, pi: ffi.ffiModel.pi);
      final type = ConnectionTypeState.find(id);
      return Row(mainAxisSize: MainAxisSize.min, children: [
        // 名字很长时截断（悬停看全名），免得窗口窄时把标题栏挤爆；标签放不下会自己变窄、横着滚
        ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 160),
          child: Tooltip(
            message: name,
            waitDuration: const Duration(milliseconds: 600),
            child: Text(name.toUpperCase(),
                maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 12, color: _kTabText)),
          ),
        ),
        if (type.isValid()) ...[
          const SizedBox(width: 6),
          Tooltip(
            message: getConnectionText(type.secure.value == ConnectionType.strSecure,
                type.direct.value == ConnectionType.strDirect, type.stream_type.value),
            // 原型只用一个盾牌：加密绿色对勾、不加密红色警示；直连还是中继看悬停说明
            child: type.secure.value == ConnectionType.strSecure
                ? ddIcon(DdIcon.shieldCheck, size: 16, color: DdColors.success)
                : ddIcon(DdIcon.shieldAlert, size: 16, color: DdColors.dangerText),
          ),
        ],
      ]);
    });
  }
}

/// 每块远程屏一个标签，选中的白底圆角上沿（照 UU / 浏览器标签）；点了切到那块屏（同一条连接、同一个窗口）。
/// 只列这个窗口管的屏（dd_remote_displays.dart），对方能拼成一个画面时最后多一个「全部显示屏」
class _DisplayTabs extends StatelessWidget {
  final RemotePage page;
  const _DisplayTabs({required this.page});

  @override
  Widget build(BuildContext context) {
    final ffi = page.ffi;
    final id = page.id;
    final owned = ddOwnedDisplays(page);
    return Obx(() {
      final pi = ffi.ffiModel.pi;
      final count = pi.displays.length;
      final current = CurrentDisplayState.find(id).value;
      if (count == 0) return const SizedBox.shrink();
      final tabs = owned.tabs(count, ddSupportAllScreens(pi));
      final screens = tabs.where((d) => d != kAllDisplayValue).length;
      // 拖出去（或右键「在新窗口打开」）：开新窗口、本窗口少掉这个标签。本窗口至少留一块屏（「全部显示屏」不算）。
      // 对方得支持多个界面各看各的屏（照 RustDesk 工具栏「在新窗口打开」的条件），不然新窗口一要画面，两个窗口就看成同一块
      bool canOpenOut(int d) =>
          pi.isSupportMultiDisplay && tabs.length > 1 && (d == kAllDisplayValue || screens > 1);

      // 窗口变窄时标签渐进变窄（原型 Remote.html「窗口缩放时标签渐进变窄」，10-27 用户：不要一下子变成图标）
      return LayoutBuilder(builder: (context, c) {
        final width = ddTabWidthFor(c.maxWidth, tabs.length);
        return SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          // 选中标签两个下角各向外弯出 8 像素（画在标签外面），两边留出这 8 像素，不然第一个、最后一个标签的弯角被滚动区裁掉
          padding: const EdgeInsets.symmetric(horizontal: 8),
          child: Row(crossAxisAlignment: CrossAxisAlignment.end, children: [
            for (final d in tabs)
              _DisplayTab(
                key: ValueKey(d),
                icon: d == kAllDisplayValue ? DdIcon.allScreens : DdIcon.screen,
                label: d == kAllDisplayValue ? translate('All screens') : ddScreenLabel(pi, d),
                selected: d == current,
                width: width,
                onTap: d == current ? null : () => openMonitorInTheSameTab(d, ffi, pi),
                onOpenOut: canOpenOut(d) ? () => ddOpenOut(page, d) : null,
              ),
          ]),
        );
      });
    });
  }
}

/// 标签后面的「铺满我所有显示器」：对方每块屏各全屏开到我的一块显示器上（dd_remote_displays.dart）。
/// 只在主窗口、对方支持多屏且有两块以上屏、本机有两块以上显示器时出现
class _FillMyDisplays extends StatelessWidget {
  final RemotePage page;
  const _FillMyDisplays({required this.page});

  @override
  Widget build(BuildContext context) {
    if (!ddOwnedDisplays(page).isMain) return const SizedBox.shrink();
    return Obx(() {
      final pi = page.ffi.ffiModel.pi;
      if (ddLocalScreens.value < 2 || !ddSupportAllScreens(pi)) return const SizedBox.shrink();
      return DdBarButton(
        tooltip: translate('Fill all my displays'),
        onTap: () => ddFillMyDisplays(page),
        child: ddIcon(DdIcon.myDisplays, size: 18, color: _kTabText),
      );
    });
  }
}

class _DisplayTab extends StatefulWidget {
  final DdIcon icon;
  final String label;
  final bool selected;

  /// 标签宽（[ddTabWidthFor] 按窗口宽算的）；窄于 [kDdTabTextMinWidth] 只画图标，名字放到悬停提示里
  final double width;
  bool get compact => width < kDdTabTextMinWidth;
  final VoidCallback? onTap;

  /// 拖出标题栏松手、或右键菜单「在新窗口打开」时调用；null 表示不能拖出（只有一块屏）
  final VoidCallback? onOpenOut;
  const _DisplayTab(
      {super.key,
      required this.icon,
      required this.label,
      required this.selected,
      required this.width,
      this.onTap,
      this.onOpenOut});

  @override
  State<_DisplayTab> createState() => _DisplayTabState();
}

class _DisplayTabState extends State<_DisplayTab> {
  /// 按下以来的累计位移。挪不到 [kDdClickSlop] 算点击（Flutter 对鼠标 2 像素就判成拖动，点的时候手一抖标签就点不动）；
  /// 超过了才算拖，标签跟着鼠标走，往下拖出标题栏（超过 [_dragOut]）松手就开新窗口
  Offset _pan = Offset.zero;
  bool _dragging = false;
  static const _dragOut = 24.0;

  /// 越过 [_dragOut] 时跟在鼠标旁边的提示：标签本身被标签栏的滚动区裁掉、拖下去就看不见了，告诉用户松手会怎样
  OverlayEntry? _hint;
  Offset _hintAt = Offset.zero;

  void _showHint(Offset global) {
    final overlay = Overlay.of(context);
    _hintAt = (overlay.context.findRenderObject() as RenderBox).globalToLocal(global) + const Offset(14, 14);
    if (_hint == null) {
      _hint = OverlayEntry(builder: (_) => _DragOutHint(at: _hintAt, icon: widget.icon));
      overlay.insert(_hint!);
    } else {
      _hint!.markNeedsBuild();
    }
  }

  void _hideHint() {
    _hint?.remove();
    _hint = null;
  }

  void _endPan() {
    _hideHint();
    setState(() {
      _pan = Offset.zero;
      _dragging = false;
    });
  }

  @override
  void dispose() {
    _hideHint();
    super.dispose();
  }

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
    final canDrag = widget.onOpenOut != null;
    return DdHover(
      builder: (context, hover) => GestureDetector(
        onTap: widget.onTap,
        onSecondaryTapDown: _menu,
        onPanUpdate: canDrag
            ? (d) {
                _pan += d.delta;
                if (!_dragging && _pan.distance < kDdClickSlop) return;
                setState(() => _dragging = true);
                _pan.dy > _dragOut ? _showHint(d.globalPosition) : _hideHint();
              }
            : null,
        onPanEnd: canDrag
            ? (_) {
                final dragging = _dragging;
                final out = _pan.dy > _dragOut;
                _endPan();
                if (!dragging) {
                  widget.onTap?.call();
                } else if (out) {
                  widget.onOpenOut!();
                }
              }
            : null,
        onPanCancel: canDrag ? _endPan : null,
        child: Transform.translate(
          offset: _dragging ? _pan : Offset.zero,
          // 照 UU（10-27 截图）：固定宽、内容靠左；选中的白底，上两角圆、下两角向外弯，和下面的画面连成一片（Chrome 的标签形状）
          child: DdChromeTab(
            selected: widget.selected,
            hover: hover,
            width: widget.width,
            padding: widget.compact
                ? const EdgeInsets.only(bottom: kDdTabTop)
                : const EdgeInsets.fromLTRB(14, 0, 14, kDdTabTop),
            child: widget.compact
                ? Tooltip(message: widget.label, child: Center(child: ddIcon(widget.icon, size: 16, color: _kTabText)))
                : Row(children: [
                    ddIcon(widget.icon, size: 16, color: _kTabText),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(widget.label,
                          overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 12, color: _kTabText)),
                    ),
                  ]),
          ),
        ),
      ),
    );
  }
}

/// 拖标签越过阈值时跟在鼠标右下的提示（深色小条，样子同轻提示 dd-toast）
class _DragOutHint extends StatelessWidget {
  final Offset at;
  final DdIcon icon;
  const _DragOutHint({required this.at, required this.icon});

  @override
  Widget build(BuildContext context) => Positioned(
        left: at.dx,
        top: at.dy,
        child: IgnorePointer(
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
            decoration: BoxDecoration(color: DdColors.ink.withAlpha(0xE6), borderRadius: BorderRadius.circular(DdRadius.md)),
            child: Row(mainAxisSize: MainAxisSize.min, children: [
              ddIcon(icon, size: 16, color: DdColors.onAccent),
              const SizedBox(width: 8),
              Text(translate('Release to open in a new window'),
                  style: const TextStyle(fontSize: 12, color: DdColors.onAccent, decoration: TextDecoration.none)),
            ]),
          ),
        ),
      );
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
      return DdBarButton(
        tooltip: translate(on ? 'Turn off the microphone' : 'Turn on the microphone'),
        onTap: () => status == VoiceCallStatus.notStarted
            ? bind.sessionRequestVoiceCall(sessionId: ffi.sessionId)
            : bind.sessionCloseVoiceCall(sessionId: ffi.sessionId),
        child: ddIcon(on || waiting ? DdIcon.mic : DdIcon.micOff,
            size: 17, color: on ? _kBlue : (waiting ? _kText3 : _kText)),
      );
    });
  }
}

/// 「控制中心」按钮：点了打开控制中心菜单（dd_control_menu.dart）
class _ControlCenter extends StatelessWidget {
  final String id;
  final FFI ffi;
  const _ControlCenter({required this.id, required this.ffi});

  @override
  Widget build(BuildContext context) {
    return DdControlMenuAnchor(
      id: id,
      ffi: ffi,
      builder: (context, toggle) => DdBarButton(
        onTap: toggle,
        child: Row(mainAxisSize: MainAxisSize.min, children: [
          ddIcon(DdIcon.controlCenter, size: 20),
          const SizedBox(width: 8),
          Text(translate('Control center'), style: const TextStyle(fontSize: 12, color: _kTabText)),
        ]),
      ),
    );
  }
}
