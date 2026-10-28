// 「控制中心」菜单（远程窗口标题栏的「控制中心」按钮和全屏时的悬浮球共用一份）：一级菜单照 UU（显示屏、切换下一个屏幕、画质、窗口、声音、
// 安全、外设、工具、更多、全屏、退出远控）。菜单项挑用 RustDesk 原工具栏的同一批（按动作编号），行为、确认框都照原样；具体放哪些等菜单 / 设置方案
// 定稿再调。不做快捷键（用户定），所以菜单项后面不标快捷键。
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/shared_state.dart';
import 'package:flutter_hbb/common/widgets/toolbar.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/desktop/widgets/dd_icons.dart';
import 'package:flutter_hbb/desktop/widgets/dd_remote_displays.dart';
import 'package:flutter_hbb/desktop/widgets/dd_remote_sync.dart';
import 'package:flutter_hbb/desktop/widgets/dd_theme.dart';
import 'package:flutter_hbb/desktop/widgets/remote_toolbar.dart' show EdgeThicknessControl;
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:flutter_hbb/utils/dd_kiosk.dart';
import 'package:get/get.dart';
import 'package:window_size/window_size.dart' as window_size;

const _kText = DdColors.ink;
const _kText2 = DdColors.ink2;
const _kText3 = DdColors.ink3;

/// 控制中心的菜单（标题栏的「控制中心」和全屏时的悬浮球共用一份）。[fullscreen]：在悬浮球上（全屏时），「全屏」换成「退出全屏」；
/// kiosk 始终全屏，不给这一项
List<Widget> ddControlMenu(BuildContext context, String id, FFI ffi, {bool fullscreen = false}) {
  // 本机显示器数按需重量（「连接时铺满我所有显示器」要两块以上才出现；开着窗口也可能插拔显示器）
  ddRefreshLocalScreens();
  final pi = ffi.ffiModel.pi;
  final count = pi.displays.length;
  final current = ddCurrentDisplay(id, count);
  final allScreens = CurrentDisplayState.find(id).value == kAllDisplayValue && count > 1;
  final mine = [
    for (final d in ddOwnedDisplaysOf(id)?.tabs(count, false) ?? List.generate(count, (i) => i))
      if (d != kAllDisplayValue) d
  ];
  final controls = toolbarControls(context, id, ffi);
  // RustDesk 的整组显示开关（画质里的真彩、声音、安全、外设、更多都从这里挑）：算一次、各处共用，免得开一次菜单算五遍
  final displayToggles = toolbarDisplayToggle(context, id, ffi);
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
                  _toggleItem(ffi, m, on: !m.value, child: Text(translate('Enable clipboard')))
                else
                  _toggleItem(ffi, m),
          ];
          return _section(items, divider: divider);
        },
      );
  // [head]：这一组的小标题（照原型「画质」「编解码」），组里一项都没有时不写。
  // [set]：选了以后不调 RustDesk 那份回调、改调它（要等存好再做别的事时用）；[label]：换掉 RustDesk 给的叫法
  Widget radios(Future<List<TRadioMenu<String>>> future,
          {String? head,
          bool divider = false,
          Future<void> Function(String value)? set,
          Widget? Function(TRadioMenu<String> m)? label}) =>
      FutureBuilder<List<TRadioMenu<String>>>(
        future: future,
        builder: (_, snap) {
          final list = snap.data ?? const <TRadioMenu<String>>[];
          return _section([
            for (final m in list)
              _pick(ffi,
                  on: m.value == m.groupValue,
                  onPressed: m.onChanged == null
                      ? null
                      : set != null
                          ? () => set(m.value)
                          : () => m.onChanged!(m.value),
                  child: label?.call(m) ?? m.child),
          ], divider: divider, head: head);
        },
      );
  Widget sub(DdIcon icon, String label, List<Widget> children) => SubmenuButton(
        menuStyle: _menuStyle,
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
    // 第一项是本窗口正在看的屏，子菜单是这块屏的分辨率 +「连接时铺满我所有显示器」（照 UU，原型 RemoteMulti.html）；
    // 切屏点标签或「切换下一个屏幕」
    if (allScreens)
      sub(DdIcon.allScreens, translate('All screens'), [_fillOnConnect(ffi)])
    else
      sub(DdIcon.monitor, current < 0 ? translate('Monitor') : ddScreenLabel(pi, current),
          [_Resolutions(ffi: ffi), _fillOnConnect(ffi, divider: true)]),
    // 只在本窗口的屏之间切（拖到别的窗口的不算，「全部显示屏」不算）
    item(DdIcon.arrowLeftRight, translate('Switch to next screen'),
        mine.length > 1 ? () => openMonitorInTheSameTab(mine[(mine.indexOf(current) + 1) % mine.length], ffi, pi) : null),
    _divider,
    // 画质（原型 RemoteMenu.html，用户 10-27 定方案 B，功能以 RustDesk 为准）：RustDesk 的四档画质（点「自定义…」照它选中并打开
    // 自定义画质对话框）、帧率（只有自定义时能点）、色彩（RustDesk 的「真彩」开关拆成两项）；编码不放
    sub(DdIcon.eye, translate('Image Quality'), [
      radios(toolbarImageQuality(context, id, ffi), label: (m) {
        final key = _kQualityLabels[m.value];
        return key == null ? null : Text(translate(key));
      }),
      _fpsGroup(id, ffi),
      _colorMode(ffi, displayToggles),
    ]),
    sub(DdIcon.panelTop, translate('Window'), [
      // 显示方式：做的事同 RustDesk 那份回调（存进这台机器的配置、本窗口按新方式重排），存好后再让同一台机器的其他窗口跟上
      radios(toolbarViewStyle(context, id, ffi), set: (v) async {
        await bind.sessionSetViewStyle(sessionId: ffi.sessionId, value: v);
        await ffi.canvasModel.updateViewStyle();
        ddSyncPeerWindows(id);
      }),
      _scrollStyle(ffi),
    ]),
    sub(DdIcon.volume, translate('Sound'), [
      toggles(displayToggles, [kShortcutActionToggleMute]),
    ]),
    sub(DdIcon.lock, translate('Security'), [
      ...security,
      toggles(displayToggles, [kShortcutActionToggleLockAfterSessionEnd],
          divider: security.isNotEmpty),
    ]),
    sub(DdIcon.mouse, translate('Peripherals'), [
      _mouseMode(ffi),
      _divider,
      toggles(displayToggles,
          [kShortcutActionToggleDisableClipboard, kShortcutActionToggleEnableFileCopyPaste]),
      // 反转滚轮、交换左右键：RustDesk 原样就是按机器存的（菜单项照它的工具栏）
      if (ffi.ffiModel.keyboard) _reverseWheel(ffi),
      for (final m in toolbarKeyboardToggles(ffi))
        if (m.actionId == kShortcutActionToggleSwapLeftRightMouse)
          _toggleItem(ffi, m),
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
      toggles(displayToggles, [kShortcutActionToggleQualityMonitor], divider: more.isNotEmpty),
      // 专业模式的主控端只有 Linux（专业窗口、硬解和直接上屏都按 Linux 写的）；别的平台点了也不会发起，不放这一项
      if (isLinux) ...[
        _divider,
        _act(ffi,
            icon: DdIcon.zap,
            onPressed: () => bind.sessionDdproRetry(sessionId: ffi.sessionId),
            child: Text(translate('Switch to pro mode'))),
      ],
    ]),
    _divider,
    if (!kDdKiosk)
      fullscreen
          ? item(DdIcon.minimize, translate('Exit Fullscreen'), () => stateGlobal.setFullscreen(false))
          : item(DdIcon.maximize, translate('Fullscreen'), () => stateGlobal.setFullscreen(true)),
    item(DdIcon.logOut, translate('End remote control'), () => closeConnection(id: id), color: DdColors.dangerText),
  ];
}

Widget _mouseMode(FFI ffi) {
  final reason = ddRemoteMouseUnavailable(ffi);
  return Builder(
    builder: (_) {
      // 按此刻实际在用的标（不是记住的）：记住被控端鼠标、这次却没打开（用不了、按 Ctrl+Alt 临时退出、出错被 RustDesk 关掉）时实际是主控端鼠标
      final remote = ffi.inputModel.relativeMouseMode.value;
      void choose(bool? v) async {
        if (v == null) return;
        // 开不成（画面尺寸还没量好、对端不支持；RustDesk 自己会提示）就不记，免得以后每次连上都再试、再提示
        if (!ffi.inputModel.setRelativeMouseMode(v) && v) return;
        await bind.sessionSetFlutterOption(sessionId: ffi.sessionId, k: kDdMouseMode, v: v ? 'remote' : '');
        ddSyncPeerWindows(ffi.id);
      }

      // 每项下面一行说明（照原型）。外面的 width: 0 让说明不把菜单撑宽：量宽度时它算 0，排版时被 stretch 拉到菜单宽、按菜单宽折行
      Widget note(String text) => SizedBox(
            width: 0,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(_kItemTextIndent, 0, 10, 6),
              child: Text(text, style: const TextStyle(fontSize: 12, height: 1.5, color: _kText2)),
            ),
          );

      return _group([
        _head(translate('Mouse mode')),
        _pick(ffi, on: !remote, onPressed: () => choose(false), child: Text(translate("Use the controller's mouse"))),
        note(translate('Better pointer response; some software or games may not work')),
        _pick(ffi,
            on: remote,
            onPressed: reason.isEmpty ? () => choose(true) : null,
            child: Text(translate("Use the controlled side's mouse"))),
        // 能用时说明特点、怎么退出；用不了时写原因。退出的快捷键照 RustDesk（relative_mouse_model.dart）：macOS 是 Cmd+G，其它是 Ctrl+Alt
        note(reason.isNotEmpty
            ? reason
            : translate(
                'Better compatibility, but the pointer display is affected by network latency; press {${isMacOS ? 'Cmd+G' : 'Ctrl+Alt'}} to exit')),
      ]);
    },
  );
}

const _divider = Divider(height: 9);

/// 把几个菜单项竖着排成一组放进子菜单（拉满菜单宽）
Widget _group(List<Widget> children) =>
    Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: children);

/// 一组选项：[items] 一项都没有时整组不画（[divider] 前面的分隔线、[head] 小标题也不留，免得剩一条孤零零的线）
Widget _section(List<Widget> items, {bool divider = false, String? head}) => _group([
      if (items.isNotEmpty) ...[
        if (divider) _divider,
        if (head != null) _head(head),
      ],
      ...items,
    ]);

/// RustDesk 的开关项（TToggleMenu）：[on] 默认取它当前的值，点了调它的回调、传相反的值；[child] 默认用它给的叫法
Widget _toggleItem(FFI ffi, TToggleMenu m, {bool? on, Widget? child}) => _pick(ffi,
    multi: true,
    on: on ?? m.value,
    onPressed: m.onChanged == null ? null : () => m.onChanged!(!m.value),
    child: child ?? m.child);

/// 控制中心里可以选中的一项（照 UU，不用复选框、单选框）：单选（几个里选一个）选中时文字前一个黑点，
/// 多选（开关，[multi]）选中时左边一个对勾，都在 [_dotSlot] 那一列。
/// 点了先刷新鼠标位置（照 RustDesk 的复选 / 单选菜单项），再调用它背后的同一个回调（复选：传相反的值；单选：传这一项的值）。
/// [extra]：右边的灰字
Widget _pick(FFI ffi,
    {required bool on, required VoidCallback? onPressed, required Widget child, Widget? extra, bool multi = false}) {
  return MenuItemButton(
    leadingIcon: _dotSlot(on, check: multi),
    onPressed: onPressed == null
        ? null
        : () {
            ffi.inputModel.refreshMousePos();
            onPressed();
          },
    trailingIcon: extra,
    child: child,
  );
}

/// 选中标记的槽（原型 dd-mi--on：文字前一个 5 像素黑点，文字不变色、右边不打勾）。和动作项的图标同宽（18），
/// 放在图标那一列：图标项和可选项混排时文字对齐；没选中的、和它们同组的普通项（如「其他分辨率」）放一个空槽。
/// [check]：开关项（原型 dd-mi--check），选中画 14 像素的 Lucide check，不画点
Widget _dotSlot(bool on, {bool check = false}) => SizedBox(
      width: 18,
      height: 18,
      child: !on
          ? null
          : check
              ? Center(child: ddIcon(DdIcon.check, size: 14, color: _kText, strokeWidth: 2))
              : Center(
                  child:
                      Container(width: 5, height: 5, decoration: const BoxDecoration(color: _kText, shape: BoxShape.circle))),
    );

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

/// 画质四档照原型的叫法（RustDesk 原来叫「画质最优化 / 平衡 / 速度最优化 / 自定义」）
const _kQualityLabels = {
  kRemoteImageQualityBest: 'Prefer image quality',
  kRemoteImageQualityBalanced: 'Balanced image quality',
  kRemoteImageQualityLow: 'Prefer speed',
  kRemoteImageQualityCustom: 'Custom...',
};

/// 帧率这一组：RustDesk 的帧率只在「自定义」画质里有（自定义画质对话框的帧率滑块，按机器记 custom-fps）；
/// 前三档画质帧率是被控端默认的 30（RustDesk 切到这三档时也把帧率改回 30），所以不是自定义时这组灰掉、写「固定 30 帧」。
/// 自定义时点哪档就把自定义帧率设成多少（同滑块：bind.sessionSetCustomFps，存进这台机器的配置）；当前值正好是这几档之一时那档带点，
/// 用滑块调成别的值时都不带点。用公共服务器又不是直连、或对方太旧时 RustDesk 的对话框不给调帧率，这里同样灰掉
Widget _fpsGroup(String id, FFI ffi) => FutureBuilder<(bool, int, bool)>(
      future: () async {
        final custom = await bind.sessionGetImageQuality(sessionId: ffi.sessionId) == kRemoteImageQualityCustom;
        final fps =
            int.tryParse(await bind.sessionGetOption(sessionId: ffi.sessionId, arg: 'custom-fps') ?? '') ?? kDefaultFps.toInt();
        bool direct = false;
        try {
          direct = ConnectionTypeState.find(id).direct.value == ConnectionType.strDirect;
        } catch (_) {}
        final locked = (await bind.mainIsUsingPublicServer() && !direct) ||
            versionCmp(ffi.ffiModel.pi.version, '1.2.0') < 0;
        return (custom, fps, locked);
      }(),
      builder: (context, snap) {
        final data = snap.data;
        if (data == null) return const SizedBox.shrink();
        final (custom, fps, locked) = data;
        final can = custom && !locked;
        Widget rate(int v) => _pick(ffi,
            on: can && fps == v,
            onPressed: can ? () => bind.sessionSetCustomFps(sessionId: ffi.sessionId, fps: v) : null,
            child: Text(translate('{$v} FPS')));
        return _group([
          _divider,
          if (!can) ...[
            MenuItemButton(
              leadingIcon: _dotSlot(false),
              trailingIcon: Text(translate('Choose "Custom" to change'),
                  style: const TextStyle(fontSize: 12, color: _kText3)),
              onPressed: null,
              child: Text(translate('Fixed 30 FPS')),
            ),
            for (final v in const [120, 90, 60]) rate(v),
          ] else
            for (final v in const [120, 90, 60, 30]) rate(v),
        ]);
      },
    );

/// 色彩：RustDesk 的「真彩（4:4:4）」开关拆成「真彩 / 常规色彩」二选一，出现条件照它（对方够新、这次用的是 AV1 / VP9）。
/// 点没选中的那项 = 拨一下这个开关（同一个回调）
Widget _colorMode(FFI ffi, Future<List<TToggleMenu>> displayToggles) => FutureBuilder<List<TToggleMenu>>(
      future: displayToggles,
      builder: (context, snap) {
        final m = snap.data?.firstWhereOrNull((t) => t.actionId == kShortcutActionToggleTrueColor);
        if (m == null) return const SizedBox.shrink();
        final change = m.onChanged;
        return _group([
          _divider,
          _pick(ffi,
              on: m.value,
              onPressed: change == null
                  ? null
                  : () {
                      if (!m.value) change(true);
                    },
              child: Text(translate('Full color (4:4:4)'))),
          _pick(ffi,
              on: !m.value,
              onPressed: change == null
                  ? null
                  : () {
                      if (m.value) change(false);
                    },
              child: Text(translate('Regular color'))),
        ]);
      },
    );

/// 菜单项文字离菜单项左边的距离：菜单项左右边距 10 + 图标 / 选中点那一列 18 + Flutter 菜单项图标和文字之间的 12
const _kItemTextIndent = 10.0 + 18 + 12;

/// 子菜单里一组的小标题（照原型：12 号灰字，和菜单项文字左对齐）
Widget _head(String text) => Padding(
      padding: const EdgeInsets.fromLTRB(_kItemTextIndent, 6, 10, 4),
      child: Text(text, style: const TextStyle(fontSize: 12, color: _kText2)),
    );

/// 滚动方式（10-27 用户：设置页「显示」里有、控制中心没有，放进「窗口」）。照 RustDesk 工具栏「显示」里的那组
/// （remote_toolbar.dart 的 scrollStyle）：按机器存；只在原始比例 / 自定义缩放时出现，画面比窗口大（溢出）时才能选；
/// 选了「边缘滚动」下面出贴边宽度
Widget _scrollStyle(FFI ffi) => FutureBuilder<(String, String, int)>(
      future: () async {
        final view = await bind.sessionGetViewStyle(sessionId: ffi.sessionId) ?? '';
        final scroll = await bind.sessionGetScrollStyle(sessionId: ffi.sessionId) ?? '';
        // 取不到时用和 RustDesk 配置默认一样的 100
        final edge = await bind.sessionGetEdgeScrollEdgeThickness(sessionId: ffi.sessionId) ?? 100;
        return (view, scroll, edge);
      }(),
      builder: (context, snap) {
        final data = snap.data;
        if (data == null) return const SizedBox.shrink();
        final (view, scroll, edge) = data;
        if (view != kRemoteViewStyleOriginal && view != kRemoteViewStyleCustom) return const SizedBox.shrink();
        Future<void> choose(String value) async {
          await bind.sessionSetScrollStyle(sessionId: ffi.sessionId, value: value);
          ffi.canvasModel.updateScrollStyle();
          ddSyncPeerWindows(ffi.id);
        }

        return Obx(() {
          final overflow = ffi.canvasModel.imageOverflow.value;
          Widget option(String value, String label) => _pick(ffi,
              on: scroll == value, onPressed: overflow ? () => choose(value) : null, child: Text(translate(label)));
          return _group([
            _divider,
            _head(translate('Scroll Style')),
            option(kRemoteScrollStyleAuto, 'ScrollAuto'),
            option(kRemoteScrollStyleBar, 'Scrollbar'),
            option(kRemoteScrollStyleEdge, 'ScrollEdge'),
            if (scroll == kRemoteScrollStyleEdge) _EdgeThickness(ffi: ffi, initial: edge),
          ]);
        });
      },
    );

/// 这块屏的分辨率（照 RustDesk 工具栏的「分辨率」，remote_toolbar.dart 的 _ResolutionsMenu），叫法照 UU：
/// 「跟随当前显示屏」= RustDesk 的适应本地，「跟随被控端」= 原始分辨率（没改过时就是现在的），「其他分辨率」= 对方这块屏支持的。
/// 出现条件照 RustDesk：有键鼠权限、对方这块屏不止一种分辨率、不是在看「全部显示屏」；虚拟显示器的自定义分辨率不做
class _Resolutions extends StatelessWidget {
  final FFI ffi;
  const _Resolutions({required this.ffi});

  /// 本机分辨率：取窗口所在的那块显示器（菜单上写的是「当前显示屏」；铺满、拖出去的窗口多半不在主显示器上。
  /// RustDesk 原来只有 Wayland 取窗口所在的屏，其它都取主显示器）。Windows 拿到的就是物理像素；Linux 拿到的是 GTK 的逻辑尺寸，
  /// X11 乘上缩放倍数，Wayland 照 RustDesk 原样不乘。取不到再退回主显示器（Wayland 照原样不退）
  static Future<Resolution?> _local() async {
    final wayland = bind.mainCurrentIsWayland();
    if (isWindows || isLinux) {
      try {
        final screen = (await window_size.getWindowInfo()).screen;
        if (screen != null) {
          final k = isLinux && !wayland ? screen.scaleFactor : 1.0;
          return Resolution((screen.frame.width * k).round(), (screen.frame.height * k).round());
        }
      } catch (e) {
        debugPrint('dd: local resolution of the window screen: $e');
      }
      if (wayland) return null;
    }
    try {
      final d = json.decode(bind.mainGetMainDisplay());
      if (d['w'] != null && d['h'] != null) return Resolution(d['w'], d['h']);
    } catch (e) {
      debugPrint('dd: local resolution: $e');
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final model = ffi.ffiModel;
    final pi = model.pi;
    final display = pi.tryGetDisplayIfNotAllDisplay();
    final resolutions = pi.resolutions;
    final rect = model.rect;
    if (display == null || rect == null || !model.keyboard || resolutions.length < 2) return const SizedBox.shrink();
    final scale = pi.scaleOfDisplay(pi.currentDisplay);
    final cw = (rect.width / scale).toInt(), ch = (rect.height / scale).toInt();
    final ow = model.isOriginalResolutionSet ? display.originalWidth : cw;
    final oh = model.isOriginalResolutionSet ? display.originalHeight : ch;
    bool isCur(int w, int h) => w == cw && h == ch;
    bool supported(int w, int h) => resolutions.any((r) => r.width == w && r.height == h);
    // 选中的照样是可点的样子（点了不动），不支持的才灰
    VoidCallback? change(int w, int h) => isCur(w, h)
        ? () {}
        : supported(w, h)
            ? () => bind.sessionChangeResolution(
                sessionId: ffi.sessionId, display: pi.currentDisplay, width: w, height: h)
            : null;
    String size(int w, int h) => '$w × $h';
    return FutureBuilder<Resolution?>(
      future: _local(),
      builder: (context, snap) {
        final local = snap.data;
        return _group([
          if (local != null)
            _pick(ffi,
                on: isCur(local.width, local.height),
                onPressed: change(local.width, local.height),
                child: Text(translate('Match this display ({${size(local.width, local.height)}})'))),
          _pick(ffi,
              on: isCur(ow, oh),
              onPressed: change(ow, oh),
              child: Text(translate('Match the remote ({${size(ow, oh)}})'))),
          SubmenuButton(
            menuStyle: _menuStyle,
            menuChildren: [
              _kMenuMinWidth,
              for (final r in resolutions)
                _pick(ffi,
                    on: isCur(r.width, r.height), onPressed: change(r.width, r.height), child: Text(size(r.width, r.height))),
            ],
            leadingIcon: _dotSlot(false),
            child: Text(translate('Other resolutions')),
          ),
        ]);
      },
    );
  }
}

/// 「连接时铺满我所有显示器」：RustDesk 的「将我的所有显示器用于远程会话」，按机器记，下次连这台时自动铺开。
/// 出现条件照 RustDesk：对方支持多屏、本机有两块以上显示器
Widget _fillOnConnect(FFI ffi, {bool divider = false}) => Obx(
      () {
        if (ddLocalScreens.value < 2 || !ffi.ffiModel.pi.isSupportMultiDisplay) return const SizedBox.shrink();
        final on = bind.sessionGetUseAllMyDisplaysForTheRemoteSession(sessionId: ffi.sessionId) == 'Y';
        return _group([
          if (divider) _divider,
          _pick(ffi, multi: true,
              on: on,
              onPressed: () => bind.sessionSetUseAllMyDisplaysForTheRemoteSession(
                  sessionId: ffi.sessionId, value: on ? 'N' : 'Y'),
              child: Text(translate('Fill all my displays when connecting'))),
        ]);
      },
    );

/// 边缘滚动的贴边宽度：RustDesk 的滑杆，拖动时照它的做法存进这台机器的配置、马上生效
class _EdgeThickness extends StatefulWidget {
  final FFI ffi;
  final int initial;
  const _EdgeThickness({required this.ffi, required this.initial});

  @override
  State<_EdgeThickness> createState() => _EdgeThicknessState();
}

class _EdgeThicknessState extends State<_EdgeThickness> {
  late double _value = widget.initial.toDouble();

  @override
  Widget build(BuildContext context) => EdgeThicknessControl(
        value: _value.clamp(EdgeThicknessControl.kMin, EdgeThicknessControl.kMax),
        onChanged: (v) async {
          setState(() => _value = v);
          final px = v.round();
          await bind.sessionSetEdgeScrollEdgeThickness(sessionId: widget.ffi.sessionId, value: px);
          widget.ffi.canvasModel.updateEdgeScrollEdgeThickness(px);
          ddSyncPeerWindows(widget.ffi.id);
        },
      );
}

/// 反转滚轮：RustDesk 原样按机器存，没存过时跟用户默认值（照它工具栏的同一项）
Widget _reverseWheel(FFI ffi) {
  final v = bind.sessionGetReverseMouseWheelSync(sessionId: ffi.sessionId) ?? '';
  final on = v == 'Y' || (v == '' && bind.mainGetUserDefaultOption(key: kKeyReverseMouseWheel) == 'Y');
  return _pick(
    ffi,
    multi: true,
    on: on,
    onPressed: ffi.ffiModel.viewOnly
        ? null
        : () => bind.sessionSetReverseMouseWheel(sessionId: ffi.sessionId, value: on ? 'N' : 'Y'),
    child: Text(translate('Reverse mouse wheel')),
  );
}

/// 控制中心菜单的样式：底色、圆角、阴影取全局主题里的菜单样式（dd_theme.dart 的 menuTheme；MenuAnchor 逐项回落到它），
/// 这里只写与它不同的：四边都留 6（全局只留上下）
const _menuStyle = MenuStyle(padding: WidgetStatePropertyAll(EdgeInsets.all(6)));

/// 菜单最少 230 宽（照原型）。MenuStyle 的 minimumSize 不管用：Flutter 3.24 的菜单面板外面包了一层只约束竖向的
/// UnconstrainedBox，子菜单还没法关掉它；所以每个菜单开头放一个这么宽、高 0 的占位，菜单按最宽的一项定宽
const _kMenuMinWidth = SizedBox(width: 230 - 12);

/// 控制中心菜单项的样式（照原型 RemoteMenu：行高 36、圆角悬停块、字 14），套在菜单的 MenuAnchor 外面，子菜单也跟着用。
/// 子菜单右边的箭头是 Flutter 写死的 Material 实心三角，换不成原型的细箭头，只能调成次要文字的灰色
Widget _menuTheme(BuildContext context, {required Widget child}) => Theme(
      data: Theme.of(context).copyWith(
        menuButtonTheme: MenuButtonThemeData(
          style: ButtonStyle(
            visualDensity: VisualDensity.standard,
            minimumSize: const WidgetStatePropertyAll(Size(0, 36)),
            padding: const WidgetStatePropertyAll(EdgeInsets.symmetric(horizontal: 10)),
            textStyle: const WidgetStatePropertyAll(TextStyle(fontSize: 14, fontWeight: FontWeight.normal)),
            iconColor: const WidgetStatePropertyAll(_kText2),
            overlayColor: const WidgetStatePropertyAll(DdColors.hover),
            shape: WidgetStatePropertyAll(RoundedRectangleBorder(borderRadius: BorderRadius.circular(DdRadius.md))),
          ),
        ),
      ),
      child: child,
    );

/// 打开控制中心菜单的锚点（标题栏的「控制中心」按钮和全屏时的悬浮球共用）。[builder] 画点了就开 / 关菜单的那一块，[toggle] 是开关；
/// [onOpen] / [onClose] 是菜单开了 / 关了的通知（悬浮球据此决定缩不缩回去）。[fullscreen]：同 [ddControlMenu]
class DdControlMenuAnchor extends StatefulWidget {
  final String id;
  final FFI ffi;
  final bool fullscreen;
  final VoidCallback? onOpen;
  final VoidCallback? onClose;
  final Widget Function(BuildContext context, VoidCallback toggle) builder;
  const DdControlMenuAnchor({
    super.key,
    required this.id,
    required this.ffi,
    required this.builder,
    this.fullscreen = false,
    this.onOpen,
    this.onClose,
  });

  @override
  State<DdControlMenuAnchor> createState() => _DdControlMenuAnchorState();
}

class _DdControlMenuAnchorState extends State<DdControlMenuAnchor> {
  final _controller = MenuController();

  // 点开时才生成菜单内容、开着期间不变：RustDesk 生成这些菜单项时会顺带登记动作、取当前值，没必要每次重建都做一遍
  List<Widget> _children = const [];

  void _toggle() {
    if (_controller.isOpen) return _controller.close();
    final children = ddControlMenu(context, widget.id, widget.ffi, fullscreen: widget.fullscreen);
    setState(() => _children = children);
    _controller.open();
  }

  @override
  Widget build(BuildContext context) {
    return _menuTheme(
      context,
      child: MenuAnchor(
        controller: _controller,
        alignmentOffset: const Offset(0, 6),
        style: _menuStyle,
        menuChildren: _children,
        onOpen: widget.onOpen,
        onClose: widget.onClose,
        child: widget.builder(context, _toggle),
      ),
    );
  }
}
