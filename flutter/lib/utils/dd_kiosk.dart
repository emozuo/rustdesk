// DreamDesk kiosk（dreamdesk/docs/K-kiosk方案.md）：程序以 `--kiosk` 启动时和平常不一样的几处
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/utils/multi_window_manager.dart';

/// 本进程是不是以 `--kiosk` 启动的。读进程自己的启动参数、不由 main() 设：远程窗口跑在同一进程的另一个 Dart 环境里，
/// main() 设的全局变量它看不到。kiosk 只有 Linux 版
final bool kDdKiosk = isLinux && _cmdline().contains('--kiosk');

List<String> _cmdline() {
  try {
    return File('/proc/self/cmdline').readAsStringSync().split('\u0000');
  } catch (_) {
    return const [];
  }
}

/// 远程窗口 → 主窗口：这次连接因为出错结束了（参数是 JSON：reason 给人看的原因，manual 是不是被人手动断开的）
const String kWindowDdKioskSessionEnded = 'dd_kiosk_session_ended';

/// 远程窗口里调用（提示框入口）：kiosk 下远程窗口不自己重试，连接类的错误（连接出错、登录被拒、要票被拒）和没人能填的口令框
/// 一出来，就把原因交给主窗口的状态页、关掉这个连接，由状态页过一会儿重新问绑定再连——座位改绑了能跟上，被拒时不会每次重试都
/// 要一张票。返回 true 表示已接手、不弹提示框
bool ddKioskTakeOverMsgBox(
    String type, String title, String text, String peerId) {
  if (!kDdKiosk) return false;
  final String reason;
  if (type == 'error' &&
      (title == 'Connection Error' ||
          title == 'Login Error' ||
          title == 'Connection authorization failed')) {
    reason = translate(text);
  } else if (type == 'input-password' || type == 're-input-password') {
    reason = translate('Connection authorization failed');
  } else {
    return false;
  }
  // 「Closed manually by …」：控制台或工作站上的人断开的，RustDesk 自己也不重连；状态页同样不自动再连
  final manual = text.toLowerCase().contains('manually');
  () async {
    await rustDeskWinManager.call(WindowType.Main, kWindowDdKioskSessionEnded,
        jsonEncode({'reason': reason, 'manual': manual}));
    closeConnection(id: peerId);
  }();
  return true;
}

class DdKioskSessionEnd {
  final String reason;
  final bool manual;
  DdKioskSessionEnd(this.reason, this.manual);
}

/// 主窗口收到的最近一次「连接出错结束」；状态页看到远程窗口关了时取走它，取不到就是用户自己断开的
final ValueNotifier<DdKioskSessionEnd?> ddKioskLastEnd = ValueNotifier(null);

/// 主窗口的窗口消息处理里调用
void ddKioskOnSessionEnded(dynamic args) {
  try {
    final m = jsonDecode(args as String) as Map<String, dynamic>;
    ddKioskLastEnd.value =
        DdKioskSessionEnd(m['reason'] as String? ?? '', m['manual'] == true);
  } catch (e) {
    debugPrint('dd-kiosk: bad session-ended message: $e');
  }
}
