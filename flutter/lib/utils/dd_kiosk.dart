// DreamDesk kiosk（dreamdesk/docs/K-kiosk方案.md）：程序以 `--kiosk` 启动时和平常不一样的几处
import 'dart:io';

import 'package:flutter_hbb/common.dart';

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

/// kiosk 下远程窗口报的错一律自动重试：RustDesk 对工作站离线、连接失败等不重试，弹框会一直挂在全屏画面上
bool ddKioskRetry(String type, bool hasRetry) =>
    hasRetry || (kDdKiosk && type == 'error');

/// RustDesk 的重试间隔每次翻倍、不封顶；kiosk 下最长 60 秒，断网久了恢复后不用干等
int ddKioskReconnectDelay(int next) => kDdKiosk && next > 60 ? 60 : next;
