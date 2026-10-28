// 受管版被控端「本机被控中」面板的数据（用户 10-28 定：去掉连接管理小窗，改成主窗口里的面板，原型 HostPanel / HostCollapsed）。
// 连接信息在连接管理器进程里（窗口永远藏着），主窗口每秒经本机通道问一次（src/ddpro/host_panel.rs）；问不通就当没有。
// 主窗口首页只用 [ddHost]：有连接时展开状态下整块换成 DdHostPanel，收起时左栏顶上放 DdHostCard（dd_host_panel.dart）。
import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_hbb/models/platform_model.dart';

/// 一条连进来的会话
class DdHostClient {
  final int id;
  /// 对方电脑名（对方自报，只用来显示）
  final String device;
  /// 对方登录的账号名（票里签名的）
  final String account;
  final String platform;
  final bool isFile;
  final bool isTerminal;
  final bool isCamera;
  final bool isPortForward;
  /// 连接起点（Unix 秒）
  final int since;
  const DdHostClient({
    required this.id,
    required this.device,
    required this.account,
    required this.platform,
    required this.isFile,
    required this.isTerminal,
    required this.isCamera,
    required this.isPortForward,
    required this.since,
  });

  static DdHostClient? fromJson(Map<String, dynamic> j) {
    if (j['disconnected'] == true) return null;
    final id = j['id'];
    if (id is! int) return null;
    return DdHostClient(
      id: id,
      device: (j['dd_device'] ?? '') as String,
      account: (j['dd_account'] ?? j['name'] ?? '') as String,
      platform: (j['dd_platform'] ?? '') as String,
      isFile: j['is_file_transfer'] == true,
      isTerminal: j['is_terminal'] == true,
      isCamera: j['is_view_camera'] == true,
      isPortForward: ((j['port_forward'] ?? '') as String).isNotEmpty,
      since: (j['dd_since'] ?? 0) as int,
    );
  }
}

class DdHostModel extends ChangeNotifier {
  Timer? _timer;
  List<DdHostClient> _clients = const [];
  bool _expanded = true;
  bool _polling = false;

  DdHostModel() {
    _timer = Timer.periodic(const Duration(seconds: 1), (_) => _poll());
    _poll();
  }

  List<DdHostClient> get clients => _clients;
  bool get hasClients => _clients.isNotEmpty;
  int get count => _clients.length;

  /// 面板展开（有人连进来默认展开，用户点收起变 false，全部断开后复位）
  bool get expanded => _expanded;
  set expanded(bool v) {
    if (_expanded == v) return;
    _expanded = v;
    notifyListeners();
  }

  /// 最早那条连接的起点，面板顶上的计时从它算
  int get since {
    final s = _clients.map((c) => c.since).where((s) => s > 0);
    return s.isEmpty ? 0 : s.reduce((a, b) => a < b ? a : b);
  }

  Future<void> _poll() async {
    // 上一次还没问完就跳过，免得 IPC 慢的时候堆起来
    if (_polling) return;
    _polling = true;
    try {
      final next = _parse(await bind.mainGetCommon(key: 'dd-host-list'));
      final changed = next.length != _clients.length || next.any((n) => !_clients.any((c) => c.id == n.id));
      if (_clients.isEmpty && next.isNotEmpty) _expanded = true;
      _clients = next;
      // 有连接时每秒通知一次，面板上的计时靠它走
      if (changed || next.isNotEmpty) notifyListeners();
    } catch (e) {
      debugPrint('dd host poll failed: $e');
    } finally {
      _polling = false;
    }
  }

  List<DdHostClient> _parse(String s) {
    final v = jsonDecode(s);
    if (v is! List) return const [];
    final out = <DdHostClient>[];
    for (final j in v) {
      final c = j is Map<String, dynamic> ? DdHostClient.fromJson(j) : null;
      if (c != null) out.add(c);
    }
    return out;
  }

  /// 断开某个连接；不带编号是全部
  Future<void> disconnect([int? id]) async {
    await bind.mainSetCommon(key: 'dd-host-close', value: '${id ?? -1}');
    await _poll();
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }
}

/// 主窗口里用的单例，第一次用到才开始问
final ddHost = DdHostModel();

/// 连接管理器进程（窗口藏着）里的几处行为，server_model.dart 里只留调用
class DdHostCm {
  DdHostCm._();
  static bool _pulled = false;

  /// 第一个连接到来时把主窗口拉出来，只拉一次：用户把主窗口关了，同一批连接期间不再反复拉
  static void onClientAdded() {
    if (_pulled) return;
    _pulled = true;
    bind.mainSetCommon(key: 'dd-host-open-main', value: '');
  }

  /// 每个周期告诉它现在还有几个连着的；一个都没有了，下一批连接再拉
  static void onTick(int active) {
    if (active == 0) _pulled = false;
  }

  /// 受管版没有小窗，来电自动接听，与专业模式一致（对方已凭连接票通过验证）
  static void onIncomingVoiceCall(int clientId) {
    bind.cmHandleIncomingVoiceCall(id: clientId, accept: true);
  }
}
