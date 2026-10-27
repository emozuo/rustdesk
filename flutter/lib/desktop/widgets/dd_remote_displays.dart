// 远程窗口的多块屏（10-27 用户定，原型 dreamdesk/design/prototype/RemoteMulti.html）：
// - 每个窗口只管自己那几块屏：连接的第一个窗口（主窗口）默认管全部，从它拖出去 / 铺开出去的窗口各管一块；
//   拖出去的窗口开出来时告诉主窗口「这块归我了」，关掉时告诉它「还给你」，主窗口的标签跟着少 / 多。
//   新窗口用的是 RustDesk 的「显示器开到新窗口」（同一条连接、不另连），它带着主窗口的窗口号（RemotePage.tabWindowId）。
// - 「全部显示屏」（RustDesk 的所有屏拼成一个画面，屏号 kAllDisplayValue）是最后一个特殊标签，也能拖出去。
// - 「铺满我所有显示器」：对方第 i 块屏全屏开到我的第 i 块显示器（照 RustDesk 的 tryUseAllMyDisplaysForTheRemoteSession，
//   它只在连上时按开关做一次；这里是标题栏上点了立刻做）。
import 'dart:convert';
import 'dart:math' as math;

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/shared_state.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/desktop/pages/remote_page.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:get/get.dart';

const _kClaim = 'dd_claim_display';
const _kReturn = 'dd_return_display';

/// 一个窗口管的屏。主窗口记「被别的窗口拿走的」（对方加了屏自动多出标签），拖出来的窗口记「自己的」
class DdOwnedDisplays {
  /// 拖出来的窗口：主窗口的窗口号；主窗口为空
  final int? originWindowId;
  final RxSet<int> _set;
  DdOwnedDisplays._(this.originWindowId, Set<int> set) : _set = RxSet<int>(set);

  bool get isMain => originWindowId == null;

  /// 这个窗口的标签（屏号从小到大，「全部显示屏」最后）。[count] 是对方的屏数，[allTab] 是对方支持拼成一个画面
  List<int> tabs(int count, bool allTab) {
    final all = [for (var i = 0; i < count; i++) i, if (allTab) kAllDisplayValue];
    return isMain ? [for (final d in all) if (!_set.contains(d)) d] : [for (final d in all) if (_set.contains(d)) d];
  }
}

final Map<String, DdOwnedDisplays> _owned = {};

/// 这个窗口里连 [page] 那台机器的屏归属（第一次问时按 RemotePage 的参数建：带着来源窗口号的是拖出来的，管它开出来的那块）
DdOwnedDisplays ddOwnedDisplays(RemotePage page) => _owned.putIfAbsent(page.id, () {
      final origin = page.tabWindowId;
      if (origin == null) return DdOwnedDisplays._(null, {});
      final child = DdOwnedDisplays._(origin, {page.display ?? 0});
      _tell(origin, _kClaim, page.id, page.display ?? 0);
      return child;
    });

/// 按机器号找（控制中心、悬浮球只有机器号）；还没建过就当主窗口、管全部
DdOwnedDisplays? ddOwnedDisplaysOf(String id) => _owned[id];

/// 对方支持把所有屏拼成一个画面（照 RustDesk 工具栏 supportIndividualWindows + 两块以上屏）
bool ddSupportAllScreens(PeerInfo pi) => pi.isSupportMultiDisplay && pi.displays.length > 1;

/// 这个窗口在看的屏；-1 表示不在看某一块（RustDesk 的「所有屏」是 -1，屏变少后记着的号也可能越界），这时标签都不选中、
/// 控制中心不写屏号（照 RustDesk 切屏按钮的范围判断）
int ddCurrentDisplay(String id, int count) {
  final d = CurrentDisplayState.find(id).value;
  return d >= 0 && d < count ? d : -1;
}

/// 第 [i] 块屏的名字：「显示屏 1」，被控端报了显示器型号就带上（照原型「显示屏 1（VX2723）」；括号经翻译，中文用全角）
String ddScreenLabel(PeerInfo pi, int i) {
  final name = translate('Screen {${i + 1}}');
  final model = i < pi.displays.length ? pi.displays[i].ddLabel : '';
  return model.isEmpty ? name : '$name${translate(' ({$model})')}';
}

void _tell(int windowId, String method, String id, int display) {
  DesktopMultiWindow.invokeMethod(windowId, method, jsonEncode({'id': id, 'display': display})).catchError((e) {
    // 主窗口已经关了：没有人要还，算了
    debugPrint('dd: $method to window $windowId failed: $e');
  });
}

/// 跟着本窗口的标签走（窗口建起来时调一次，RustDesk 的 ConnectionTabPage 里一行钩子）：
/// - 新来的远程页马上建好归属，拖出来的窗口当场向主窗口报到。不能等标题栏第一次画：铺开出去的窗口很快全屏、标题栏收起，
///   可能一直不画，主窗口就一直不知道这块屏被拿走了
/// - 不见了的（关窗口时标签被清空）：拖出来的把屏还给主窗口，记录删掉。RustDesk 关掉的远程窗口不销毁、藏起来留给下一次连接复用，
///   不删的话复用时沿用旧的归属：主窗口少几个标签，或者拖出来的窗口被当成主窗口、不报到
void ddWatchDisplays(DesktopTabController tabController) {
  void sync(DesktopTabState s) {
    final present = <String>{};
    for (final t in s.tabs) {
      if (t.page is RemotePage) {
        present.add(t.key);
        ddOwnedDisplays(t.page as RemotePage);
      }
    }
    for (final id in _owned.keys.where((k) => !present.contains(k)).toList()) {
      final owned = _owned.remove(id)!;
      final origin = owned.originWindowId;
      if (origin == null) continue;
      for (final d in owned._set) {
        _tell(origin, _kReturn, id, d);
      }
    }
  }

  sync(tabController.state.value);
  tabController.state.listen(sync);
  ddRefreshLocalScreens();
}

/// 收到别的窗口发来的「屏认领 / 归还」（窗口消息入口是 dd_remote_sync.dart 的 ddHandleRemoteWindowCall），是就处理、返回真
bool ddHandleDisplayCall(MethodCall call, DesktopTabController tabController) {
  if (call.method != _kClaim && call.method != _kReturn) return false;
  final args = jsonDecode(call.arguments);
  final String id = args['id'];
  final int display = args['display'];
  final tab = tabController.state.value.tabs.firstWhereOrNull((t) => t.key == id);
  if (tab == null || tab.page is! RemotePage) return true;
  final page = tab.page as RemotePage;
  final owned = ddOwnedDisplays(page);
  if (!owned.isMain) return true;
  if (call.method == _kReturn) {
    owned._set.remove(display);
    return true;
  }
  owned._set.add(display);
  // 被拿走的正是这个窗口在看的：换去看本窗口剩下的第一块（照拖出时的做法，免得两个窗口看同一块）
  final ffi = page.ffi;
  final pi = ffi.ffiModel.pi;
  if (pi.currentDisplay == display) {
    final rest = owned.tabs(pi.displays.length, ddSupportAllScreens(pi));
    if (rest.isNotEmpty) openMonitorInTheSameTab(rest.first, ffi, pi);
  }
  return true;
}

/// 把标签 [display] 拖出去：先在本窗口记下（标签马上少掉，不等新窗口来报），正在看它就先换一块，再开新窗口
void ddOpenOut(RemotePage page, int display) {
  final owned = ddOwnedDisplays(page);
  final ffi = page.ffi;
  final pi = ffi.ffiModel.pi;
  if (owned.isMain) owned._set.add(display);
  if (pi.currentDisplay == display) {
    final rest = owned.tabs(pi.displays.length, ddSupportAllScreens(pi));
    if (rest.isNotEmpty) openMonitorInTheSameTab(rest.first, ffi, pi);
  }
  openMonitorInNewTabOrWindow(display, page.id, pi);
}

/// 本机显示器块数（「铺满我所有显示器」按钮和「连接时铺满」要两块以上才出现）。开着窗口也可能插拔显示器，
/// 所以不是只量一次：窗口建起来、鼠标进标题栏、开控制中心时各重量一次（[ddRefreshLocalScreens]）
final RxInt ddLocalScreens = 1.obs;

void ddRefreshLocalScreens() {
  getScreenRectList().then((l) => ddLocalScreens.value = l.length).catchError((e) {
    debugPrint('dd: count local screens: $e');
    return 0;
  });
}

/// 「铺满我所有显示器」：照 RustDesk 的 tryUseAllMyDisplaysForTheRemoteSession——本窗口切到对方第 1 块屏、移到我的第 1 块显示器
/// 全屏，对方第 i 块各开一个窗口移到我的第 i 块显示器全屏，块数取两边少的。只在主窗口做
Future<void> ddFillMyDisplays(RemotePage page) async {
  final owned = ddOwnedDisplays(page);
  final ffi = page.ffi;
  final pi = ffi.ffiModel.pi;
  if (!owned.isMain || !pi.isSupportMultiDisplay || pi.displays.length <= 1) return;
  final screens = await getScreenRectList();
  if (screens.length <= 1) return;
  // 第 1 块屏要是先被拖出去了，收回来由本窗口看（那个窗口照旧开着，看的是同一块）
  owned._set.remove(0);
  openMonitorInTheSameTab(0, ffi, pi);
  await tryMoveToScreenAndSetFullscreen(screens[0]);
  final n = math.min(pi.displays.length, screens.length);
  for (var i = 1; i < n; i++) {
    owned._set.add(i);
    openMonitorInNewTabOrWindow(i, page.id, pi, screenRect: screens[i]);
  }
}
