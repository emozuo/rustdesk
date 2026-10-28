// 受管版远程窗口的窗口级钩子：建起来时（[ddOnRemoteWindowCreated]）、和远程窗口之间的跨窗口消息入口
// （RustDesk 的 _remoteMethodHandler 开头一行钩子调 [ddHandleRemoteWindowCall]）：
// - 同一台机器的几个窗口保持一致的设置同步（本文件）；
// - 屏的认领 / 归还（dd_remote_displays.dart，那边管屏的归属，这里只转发）；
// 另外记着「鼠标模式」（按机器记的远程鼠标 / 本地鼠标）：它的值要在这几个窗口之间同步，放在一起。
import 'dart:async';

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/desktop/pages/remote_page.dart';
import 'package:flutter_hbb/desktop/widgets/dd_remote_displays.dart';
import 'package:flutter_hbb/desktop/widgets/dd_theme.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/main.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:get/get.dart';

/// 远程窗口刚建起来时（RustDesk 的 `_ConnectionTabPageState` 构造函数里一行钩子）：开始盯着多块屏各归哪个窗口（dd_remote_displays.dart），
/// 并设窗口最小尺寸——远程窗口不能无限缩小（插件里补的 setMinimumSize，见 dreamdesk/third_party/desktop_multi_window/DREAMDESK.md）
void ddOnRemoteWindowCreated(Map<String, dynamic> params, DesktopTabController tabController) {
  ddWatchDisplays(tabController);
  WindowController.fromWindowId(params['windowId'])
      .setMinimumSize(DdSize.remoteWindowMin)
      .catchError((e) => debugPrint('dd: set minimum window size: $e'));
}

/// 远程窗口收到别的窗口发来的消息，是我们的就处理、返回真
bool ddHandleRemoteWindowCall(MethodCall call, DesktopTabController tabController) =>
    // 同一台机器的另一个窗口改了设置（[ddSyncPeerWindows]）
    _handleSettingsSync(call, tabController) || ddHandleDisplayCall(call, tabController);

/// 鼠标模式按机器记（dreamdesk/docs/M-菜单设置方案.md）：使用主控端鼠标 = RustDesk 原来的绝对坐标；使用被控端鼠标 = RustDesk 的相对鼠标模式
/// （本机指针锁在窗口里藏起来，只发移动量，画面上画出被控端报回来的指针）。记在这台机器的配置里，每次连上（含重连）按记住的值打开（[ddApplyRememberedMouseMode]）。
/// RustDesk 自己在出错、重连、键盘权限被收回时会关掉相对模式，Ctrl+Alt 也能临时退出（防鼠标被锁死）——这些都不改记住的值，
/// 只有在菜单里切换才改
const kDdMouseMode = 'dd-mouse-mode';

/// 不能用被控端鼠标的原因；空串表示能用
String ddRemoteMouseUnavailable(FFI ffi) {
  if (!ffi.inputModel.isRelativeMouseModeSupported) return translate("The remote side is too old for the controlled side's mouse");
  if (isLinux && bind.mainCurrentIsWayland()) return translate("The controlled side's mouse does not work on a Wayland desktop");
  if (!ffi.ffiModel.keyboard || ffi.ffiModel.viewOnly) return translate('No keyboard and mouse permission');
  return '';
}

Future<bool> _remoteMouseRemembered(FFI ffi) async =>
    (await bind.sessionGetFlutterOption(sessionId: ffi.sessionId, k: kDdMouseMode)) == 'remote';

/// 连上（含重连）拿到第一帧画面时调用：记住的是远程鼠标、又能用，就打开
Future<void> ddApplyRememberedMouseMode(FFI ffi) async {
  if (ddRemoteMouseUnavailable(ffi).isNotEmpty || !await _remoteMouseRemembered(ffi)) return;
  ffi.inputModel.setRelativeMouseMode(true);
}

// ---------------------------------------------------------------- 同一台机器的几个窗口保持一致
// 用户 10-27 定（方案 A）：屏拖成几个窗口后，在任一窗口的控制中心改了「每个窗口自己处理」的设置，同一台机器的其他窗口马上跟上。
// 几个窗口共用同一条连接，设置按机器只存一份；画质、帧率、剪贴板这类由对方执行，本来就对所有窗口生效；反转滚轮、交换左右键
// 每个鼠标事件都现读配置，也不用管。要跟上的只有本机这边按窗口应用的：显示方式（含自定义缩放）、滚动方式、贴边宽度、鼠标模式

const _kSyncSettings = 'dd_sync_settings';
final Map<String, Timer> _syncPending = {};

/// 告诉同一台机器 [id] 的其他远程窗口：设置变了，重新读一遍。拖滑杆时会连着来，150 毫秒内只发最后一次。
/// 发给所有子窗口（远程窗口之间互不知道对方是谁；不是远程窗口、或者连的不是这台的，收到了不理）
void ddSyncPeerWindows(String id) {
  _syncPending[id]?.cancel();
  _syncPending[id] = Timer(const Duration(milliseconds: 150), () async {
    _syncPending.remove(id);
    final me = kWindowId;
    List<int> windows;
    try {
      windows = await DesktopMultiWindow.getAllSubWindowIds();
    } catch (e) {
      debugPrint('dd: list windows for settings sync: $e');
      return;
    }
    for (final w in windows) {
      if (w == me) continue;
      DesktopMultiWindow.invokeMethod(w, _kSyncSettings, id).catchError((e) {
        debugPrint('dd: settings sync to window $w: $e');
      });
    }
  });
}

/// 收到别的窗口的「设置变了」（[ddHandleRemoteWindowCall] 转过来）：按这台机器现在存的设置重新应用到本窗口。
/// 远程鼠标打开只是改设置：RustDesk 只在鼠标在画面里时才把光标锁进窗口，不在这个窗口里的不会去抢鼠标
bool _handleSettingsSync(MethodCall call, DesktopTabController tabController) {
  if (call.method != _kSyncSettings) return false;
  final String id = call.arguments;
  final tab = tabController.state.value.tabs.firstWhereOrNull((t) => t.key == id);
  if (tab == null || tab.page is! RemotePage) return true;
  _applySyncedSettings((tab.page as RemotePage).ffi).catchError((e) {
    debugPrint('dd: apply synced settings: $e');
  });
  return true;
}

Future<void> _applySyncedSettings(FFI ffi) async {
  await ffi.canvasModel.updateViewStyle();
  await ffi.canvasModel.updateScrollStyle();
  final edge = await bind.sessionGetEdgeScrollEdgeThickness(sessionId: ffi.sessionId);
  if (edge != null) ffi.canvasModel.updateEdgeScrollEdgeThickness(edge);
  final remote = await _remoteMouseRemembered(ffi);
  if (ffi.inputModel.relativeMouseMode.value == remote) return;
  if (!remote) {
    ffi.inputModel.setRelativeMouseMode(false);
  } else if (ddRemoteMouseUnavailable(ffi).isEmpty) {
    ffi.inputModel.setRelativeMouseMode(true);
  }
}
