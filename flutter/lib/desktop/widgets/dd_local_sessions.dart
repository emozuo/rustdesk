// 受管版主窗口首页的「本机正开着的会话」（原型 HomeControlling / HomeControllingOpen）：左栏会话数角标、详情页「被控中」横条和
// 正在连接的设备列表、「显示远控窗口」「断开远控」。
// 会话从 Rust 的会话表取（src/ddpro/sessions.rs：一台一种连接一条，专业模式用的也是这一条），各窗口开在哪个子窗口里主窗口不知道，
// 「显示 / 断开」广播给所有子窗口，各窗口按自己的类型认领（remote_tab_page / file_manager_tab_page / port_forward_tab_page /
// terminal_tab_page 里各一行钩子调 [ddHandleSessionCall]）。
import 'dart:convert';

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/widgets/dialog.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/main.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/utils/multi_window_manager.dart';

/// 会话的类型（会话类型小标签）
enum DdSessionKind { remote, file, port, terminal, camera }

/// 本机开着的一个会话：连的是哪台、什么类型、从什么时候开始（连接时长从这里每秒算）
class DdLocalSession {
  final String id;
  final DdSessionKind kind;
  final DateTime startedAt;
  const DdLocalSession(this.id, this.kind, this.startedAt);

  String get label => translate(switch (kind) {
        DdSessionKind.remote => 'Remote control',
        DdSessionKind.file => 'File transfer',
        DdSessionKind.port => 'Port mapping',
        DdSessionKind.terminal => 'Terminal',
        DdSessionKind.camera => 'View camera',
      });
}

/// 读一次本机开着的会话（同步，读的是内存里的会话表）。拿不到就当没有
List<DdLocalSession> ddReadLocalSessions() {
  try {
    final list = jsonDecode(bind.mainGetCommonSync(key: 'dd-sessions'));
    if (list is! List) return const [];
    final now = DateTime.now();
    final sessions = [
      for (final m in list)
        if (m is Map && m['id'] is String && m['secs'] is int)
          DdLocalSession(
            m['id'],
            DdSessionKind.values.firstWhere((k) => k.name == m['type'], orElse: () => DdSessionKind.remote),
            now.subtract(Duration(seconds: m['secs'])),
          ),
    ];
    // Rust 那边是哈希表，每次的顺序可能不同：排好，[ddSameSessions] 才能逐个比、列表也不跳
    sessions.sort((a, b) => a.id != b.id ? a.id.compareTo(b.id) : a.kind.index.compareTo(b.kind.index));
    return sessions;
  } catch (e) {
    debugPrint('dd-sessions: $e');
    return const [];
  }
}

/// 两次读到的是不是同一批会话（只比哪台、什么类型、开始时刻差不到两秒；每秒读一次，时刻有毫秒级的抖动）
bool ddSameSessions(List<DdLocalSession> a, List<DdLocalSession> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i].id != b[i].id ||
        a[i].kind != b[i].kind ||
        a[i].startedAt.difference(b[i].startedAt).inSeconds.abs() >= 2) {
      return false;
    }
  }
  return true;
}

/// 「08:05」「01:02:03」：连接时长
String ddDurationText(Duration d) {
  String two(int n) => n.toString().padLeft(2, '0');
  final s = d.inSeconds < 0 ? 0 : d.inSeconds;
  return '${two(s ~/ 3600)}:${two(s ~/ 60 % 60)}:${two(s % 60)}';
}

/// 主窗口发给子窗口的：参数 `{"id": 设备号, "kind": 类型名, "action": "show" | "close"}`
const kDdWindowSessionAction = 'dd_session_action';

Future<void> _broadcast(String id, DdSessionKind kind, String action) async {
  final msg = jsonEncode({'id': id, 'kind': kind.name, 'action': action});
  for (final windowId in await rustDeskWinManager.getAllSubWindowIds()) {
    try {
      await DesktopMultiWindow.invokeMethod(windowId, kDdWindowSessionAction, msg);
    } catch (e) {
      debugPrint('dd-sessions: window $windowId: $e');
    }
  }
}

/// 「显示远控窗口」：把连这台的远程窗口提到前面
Future<void> ddShowSessionWindow(String id) => _broadcast(id, DdSessionKind.remote, 'show');

/// 「断开远控」：关掉这一个会话（它所在窗口里的标签，和点标签上的关闭一样）
Future<void> ddCloseSession(DdLocalSession s) => _broadcast(s.id, s.kind, 'close');

/// 子窗口的方法处理里调：是首页发来的「显示 / 断开」就处理掉、返回真。[kind] 是这个窗口开的会话类型，别的类型不认领。
/// [close] 关一个标签（终端窗口有自己的关法：先关终端会话）；不给就照普通标签的关闭按钮做（先问审计备注再关）
Future<bool> ddHandleSessionCall(MethodCall call, DesktopTabController tabs, DdSessionKind kind,
    {Future<void> Function(String key)? close}) async {
  if (call.method != kDdWindowSessionAction) return false;
  final Map args;
  try {
    args = jsonDecode(call.arguments as String);
  } catch (_) {
    return true;
  }
  if (args['kind'] != kind.name) return true;
  final id = args['id'];
  // 终端窗口一台可以开几个标签（键是「设备号_序号」），其余窗口的键就是设备号
  final keys = [
    for (final t in tabs.state.value.tabs)
      if (t.key == id || (kind == DdSessionKind.terminal && t.key.startsWith('${id}_'))) t.key
  ];
  if (keys.isEmpty) return true;
  if (args['action'] == 'show') {
    tabs.jumpToByKey(keys.first);
    windowOnTop(kWindowId);
    return true;
  }
  for (final key in keys) {
    if (close != null) {
      await close(key);
    } else if (!await desktopTryShowTabAuditDialogCloseCancelled(id: key, tabController: tabs)) {
      tabs.closeBy(key);
    }
  }
  return true;
}
