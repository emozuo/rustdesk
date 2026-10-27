// 受管版文件传输 / 终端 / 端口映射这几个子窗口共用的外观（用户 10-27 定方案 A：只换外观，不动布局和行为）。
// 原型：dreamdesk/design/prototype/Terminal.html、PortMap.html、FileTransfer.html（样式变量 dd.css）。
// RustDesk 那边只留 `if (kDdManaged)` 的钩子；窗口、标签的逻辑照用它的，这里只管画。
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/utils/multi_window_manager.dart';
import 'package:get/get.dart';

import 'dd_icons.dart';
import 'dd_theme.dart';
import 'dd_titlebar_common.dart';
import 'tabbar_widget.dart';

/// 这四种窗口（文件传输、终端、端口映射、被控端的连接管理器）的标签栏换成 [DdSubWindowBar]（远程窗口有自己的 DdRemoteTitleBar，主窗口自己画）
bool ddUsesSubWindowBar(DesktopTabType type) =>
    type == DesktopTabType.fileTransfer ||
    type == DesktopTabType.terminal ||
    type == DesktopTabType.portForward ||
    type == DesktopTabType.cm;

const kDdSubBarHeight = DdSize.titlebarRemote;

/// 标题栏右边「新建」按钮要的东西：提示和点击（[custom] 是认不出来的部件，原样放上去）。
/// 受管版的钩子（tabbar_widget.dart）用 [ddSubBarTailOf] 把 RustDesk 给的 tail 部件换成它，标题栏本身不认 RustDesk 的部件
class DdSubBarTail {
  final String? tooltip;
  final VoidCallback? onTap;
  final Widget? custom;
  const DdSubBarTail({this.tooltip, this.onTap}) : custom = null;
  const DdSubBarTail.custom(Widget this.custom)
      : tooltip = null,
        onTap = null;
}

/// RustDesk 的 tail 只有两种：终端的 ActionIcon（新开一个终端）、另两种窗口的 AddButton（回主窗口新建连接，它的点击藏在 build 里，
/// 取不出来，只能照它再写一遍）
DdSubBarTail? ddSubBarTailOf(Widget? tail) {
  if (tail == null) return null;
  if (tail is ActionIcon) {
    return DdSubBarTail(tooltip: tail.message == null ? null : translate(tail.message!), onTap: tail.onTap);
  }
  if (tail is AddButton) {
    return DdSubBarTail(
        tooltip: translate('New Connection'),
        onTap: () => rustDeskWinManager.call(WindowType.Main, kWindowMainWindowOnTop, ''));
  }
  return DdSubBarTail.custom(tail);
}

/// 文件传输 / 终端 / 端口映射窗口的标题栏（原型 FileTransfer / Terminal / PortMap 的标题栏部分，样子和远程窗口的一致）：
/// 左边图标 + 窗口名（粗）+ 机器名（灰）；终端一直画标签（「终端 1」「终端 2」）和后面的 +，另两种窗口只在连了不止一台时才画标签（标签是机器名）；
/// 右边是窗口按钮。标签的选中、关闭、右键菜单、「新建」的动作都还是 [DesktopTab] 带进来的那些，这里只管画
class DdSubWindowBar extends StatefulWidget {
  final DesktopTabController controller;
  final TabMenuBuilder? tabMenuBuilder;

  /// RustDesk 在标题栏右边给的「新建」按钮（终端：新开一个终端；另两种：回主窗口新建连接）
  final DdSubBarTail? tail;
  final Future<bool> Function()? onClose;
  final bool showMinimize;
  final bool showMaximize;
  final bool showClose;
  const DdSubWindowBar({
    super.key,
    required this.controller,
    this.tabMenuBuilder,
    this.tail,
    this.onClose,
    this.showMinimize = true,
    this.showMaximize = true,
    this.showClose = true,
  });

  @override
  State<DdSubWindowBar> createState() => _DdSubWindowBarState();
}

class _DdSubWindowBarState extends State<DdSubWindowBar> {
  DesktopTabController get _c => widget.controller;
  bool get _isTerminal => _c.tabType == DesktopTabType.terminal;

  /// 被控端的连接管理器（原型 Cm.html）：窗口名是程序名，标签是连进来的人（TabInfo.label），不是机器
  bool get _isCm => _c.tabType == DesktopTabType.cm;

  String get _title => switch (_c.tabType) {
        DesktopTabType.fileTransfer => translate('File transfer'),
        DesktopTabType.terminal => translate('Terminal'),
        DesktopTabType.cm => bind.mainGetAppNameSync(),
        _ => translate('Port mapping'),
      };

  /// 标签对应的机器：终端的标签键是「机器_终端号」，另两种就是机器号
  String _peerOf(TabInfo t) => _isTerminal ? t.key.replaceFirst(RegExp(r'_\d+$'), '') : t.key;

  /// 终端的标签叫「终端 N」（N 是终端号，原标签文字尾巴上的 #N）；连了不止一台机器时改叫「机器名 N」
  String _tabText(TabInfo t, {required bool onePeer}) {
    if (_isCm) return t.label;
    final name = ddPeerTitle(_peerOf(t)).toUpperCase();
    if (!_isTerminal) return name;
    final n = RegExp(r'#(\d+)$').firstMatch(t.label)?.group(1);
    final base = onePeer ? translate('Terminal') : name;
    return n == null ? base : '$base $n';
  }

  Widget? _tailButton() {
    final t = widget.tail;
    if (t == null) return null;
    return t.custom ?? DdBarButton(tooltip: t.tooltip, onTap: t.onTap, child: ddIcon(DdIcon.plus, size: 16));
  }

  Widget _left(Widget? right) {
    final st = _c.state.value;
    final tabs = st.tabs;
    final peers = {for (final t in tabs) _peerOf(t)};
    final showTabs = _isTerminal ? tabs.isNotEmpty : tabs.length > 1;
    final tail = _isTerminal ? _tailButton() : null;
    final sc = st.scrollController;
    // 左边这一段占满右边按钮左边的地方，没用完的留在它右边当拖动的空白；「新建」按钮在它外面，始终贴着窗口按钮
    return Row(children: [
      Expanded(
        child: Row(children: [
          const SizedBox(width: 14),
          loadIcon(18),
          const SizedBox(width: 8),
          Text(_title, style: const TextStyle(fontSize: 12, fontWeight: FontWeight.w600, color: DdColors.titleText)),
          if (!_isCm && peers.length == 1) ...[
            const SizedBox(width: 8),
            // 机器名很长时截断，免得窗口窄时把标题栏挤爆
            ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 160),
              child: Text(ddPeerTitle(peers.first).toUpperCase(),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontSize: 12, color: DdColors.ink2)),
            ),
          ],
          if (showTabs) ...[
            Container(
                width: 1, height: 16, color: DdColors.titleSep, margin: const EdgeInsets.symmetric(horizontal: 12)),
            Flexible(
              // 标签按能用的宽渐进变窄（规则同远程窗口的屏幕标签，ddTabWidthFor），放不下最窄的才横着滚
              child: LayoutBuilder(builder: (context, c) {
                final width = ddTabWidthFor(c.maxWidth, tabs.length);
                return Listener(
                  // 滚轮横着滚标签
                  onPointerSignal: (e) {
                    if (e is PointerScrollEvent && sc.hasClients) {
                      sc.jumpTo((sc.offset + e.scrollDelta.dy).clamp(0.0, sc.position.maxScrollExtent));
                    }
                  },
                  child: ListView(
                    controller: sc,
                    scrollDirection: Axis.horizontal,
                    shrinkWrap: true,
                    // 选中标签两个下角各向外弯出 8 像素（画在标签外面），两边留出这 8 像素，不然头尾的弯角被裁掉
                    padding: const EdgeInsets.symmetric(horizontal: 8),
                    children: [
                      for (var i = 0; i < tabs.length; i++)
                        _SubTab(
                          key: ValueKey(tabs[i].key),
                          width: width,
                          label: _tabText(tabs[i], onePeer: peers.length == 1),
                          selected: i == st.selected,
                          closable: tabs[i].closable,
                          onTap: () {
                            _c.jumpTo(i);
                            tabs[i].onTap?.call();
                          },
                          onClose: () {
                            final close = tabs[i].onTabCloseButton;
                            close != null ? close() : _c.remove(i);
                          },
                          onMenu: widget.tabMenuBuilder == null
                              ? null
                              : (pos) => showRightMenu((_) => widget.tabMenuBuilder!(tabs[i].key), target: pos),
                        ),
                    ],
                  ),
                );
              }),
            ),
          ],
          if (tail != null) tail,
          // 再窄也至少留这么宽给拖动窗口
          const SizedBox(width: 16),
        ]),
      ),
      // 右边的「新建」按钮也放在拖动区里：它只有 28 高，按在上下的空白里也要能拖窗口
      if (right != null) right,
    ]);
  }

  /// 主题用全局的受管版主题（ddLightTheme），不另套（理由同远程窗口标题栏 DdRemoteTitleBar）
  @override
  Widget build(BuildContext context) {
    final right = _isTerminal ? null : _tailButton();
    return Material(
      color: DdColors.chromeRemote,
      child: Row(children: [
        Expanded(child: DdTitleDragArea(child: Obx(() => _left(right)))),
        DdWindowButtons(
          onClose: widget.onClose,
          showMinimize: widget.showMinimize,
          showMaximize: widget.showMaximize,
          showClose: widget.showClose,
        ),
      ]),
    );
  }
}

/// 一个标签（形状、宽度规则同远程窗口的屏幕标签：按窗口宽渐进变窄，选中的白底、上两角圆、下两角向外弯）；
/// 选中或鼠标在上面时右边出一个 ×。这里的标签没有图标，窄了也照样写字（末尾省略），名字放悬停提示
class _SubTab extends StatelessWidget {
  final double width;
  final String label;
  final bool selected;
  final bool closable;
  final VoidCallback onTap;
  final VoidCallback onClose;
  final void Function(Offset position)? onMenu;
  const _SubTab({
    super.key,
    required this.width,
    required this.label,
    required this.selected,
    required this.closable,
    required this.onTap,
    required this.onClose,
    this.onMenu,
  });

  @override
  Widget build(BuildContext context) {
    // 窄了左右边距跟着收（最窄 40 时还要放下 19 宽的 ×）
    final narrow = width < kDdTabTextMinWidth;
    final tab = DdHover(
      builder: (context, hover) => Listener(
        onPointerDown: (e) {
          if (e.kind == PointerDeviceKind.mouse && e.buttons == kSecondaryMouseButton) onMenu?.call(e.position);
        },
        child: GestureDetector(
          onTap: onTap,
          child: DdChromeTab(
            selected: selected,
            hover: hover,
            width: width,
            padding: narrow
                ? const EdgeInsets.fromLTRB(8, 0, 4, kDdTabTop)
                : const EdgeInsets.fromLTRB(14, 0, 8, kDdTabTop),
            child: Row(children: [
              Expanded(
                child: Text(label,
                    overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 12, color: DdColors.titleText)),
              ),
              if (closable && (selected || hover))
                GestureDetector(
                  onTap: onClose,
                  child: Padding(
                    padding: const EdgeInsets.all(3),
                    child: ddIcon(DdIcon.close, size: 13, color: DdColors.ink2),
                  ),
                ),
            ]),
          ),
        ),
      ),
    );
    return narrow ? Tooltip(message: label, child: tab) : tab;
  }
}

