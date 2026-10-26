// DreamDesk kiosk（dreamdesk/docs/K-kiosk方案.md）：程序以 `--kiosk` 启动时和平常不一样的几处
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/widgets/ddpro_mode_badge.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/utils/multi_window_manager.dart';

/// 本进程是不是以 `--kiosk` 启动的。读进程自己的启动参数、不由 main() 设：远程窗口跑在同一进程的另一个 Dart 环境里，
/// main() 设的全局变量它看不到。kiosk 只有 Linux 版
final bool kDdKiosk = isLinux && _cmdline().contains('--kiosk');

/// 受管版（产品只有受管版，dreamdesk/docs/M-受管版方案.md）：主窗口首页是「我的设备」（desktop/pages/dd_home_page.dart）
final bool kDdManaged = isDesktop && bind.mainGetCommonSync(key: 'dd-managed') == 'Y';

List<String> _cmdline() {
  try {
    return File('/proc/self/cmdline').readAsStringSync().split('\u0000');
  } catch (_) {
    return const [];
  }
}

/// 远程窗口 → 主窗口：这次连接因为出错结束了（参数是 JSON：reason 给人看的原因，manual 是不是工作站上的人断开的，
/// refused 是不是被拒）
const String kWindowDdKioskSessionEnded = 'dd_kiosk_session_ended';

/// 远程窗口里调用（提示框入口）：kiosk 下远程窗口不自己重试，连接类的错误（连接出错、登录被拒、要票被拒）和没人能填的口令框
/// 一出来，就把原因交给主窗口、关掉这个连接，由主窗口的状态卡片决定：掉线的过一会儿重连同一台，被拒的停住（再试也一样，
/// 免得每次重试都要一张票）。返回 true 表示已接手、不弹提示框
bool ddKioskTakeOverMsgBox(String type, String title, String text,
    SessionID sessionId, String peerId, void Function() reconnectViaRelay) {
  if (!kDdKiosk) return false;
  debugPrint('dd-kiosk: msgbox $type / $title: $text');
  // 直连被对端重置时 RustDesk 问要不要改走中继：没人能答，照提示框里「走中继」那个按钮做
  if (type == 'relay-hint' || type == 'relay-hint2') {
    reconnectViaRelay();
    return true;
  }
  final String reason;
  // 被拒（要票被拒、被控端不认票、没人能填的口令框）：再试也一样，状态页不自动重连
  var refused = false;
  if (type == 'error' &&
      (title == 'Connection Error' ||
          title == 'Login Error' ||
          title == 'Error' ||
          title == 'Connection authorization failed')) {
    reason = translate(text);
    refused = title != 'Connection Error' && title != 'Error';
  } else if (type == 'input-password' || type == 're-input-password') {
    reason = translate('Connection authorization failed');
    refused = true;
  } else {
    return false;
  }
  // 只有工作站上的人断开的（「Closed manually by the peer」）才停住；「… by web console」是服务端经心跳要断的
  // （收回授权、控制台单独断开等都是这一句），照掉线处理：退避后重连，授权真收回了，那次要票会被拒、停在卡片上
  final manual = text.toLowerCase().contains('manually by the peer');
  () async {
    await rustDeskWinManager.call(WindowType.Main, kWindowDdKioskSessionEnded,
        jsonEncode({'reason': reason, 'manual': manual, 'refused': refused}));
    // 和专业模式关窗同一套收尾：专业画面开着时这个窗口是藏着的，只移除标签关不掉会话和窗口（见那里的说明）；
    // 关窗走子窗口的关闭流程，会通知主窗口把它移出活动名单
    onDdproWindowEvent(sessionId, peerId, {'action': 'close'});
  }();
  return true;
}

class DdKioskSessionEnd {
  final String reason;
  final bool manual;
  final bool refused;
  DdKioskSessionEnd(this.reason, this.manual, this.refused);
}

/// 主窗口收到的最近一次「连接出错结束」；状态页看到远程窗口关了时取走它，取不到就是用户自己断开的
final ValueNotifier<DdKioskSessionEnd?> ddKioskLastEnd = ValueNotifier(null);

/// 主窗口的窗口消息处理里调用
void ddKioskOnSessionEnded(dynamic args) {
  try {
    final m = jsonDecode(args as String) as Map<String, dynamic>;
    ddKioskLastEnd.value =
        DdKioskSessionEnd(m['reason'] as String? ?? '', m['manual'] == true, m['refused'] == true);
  } catch (e) {
    debugPrint('dd-kiosk: bad session-ended message: $e');
  }
}
