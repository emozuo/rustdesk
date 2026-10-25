// DreamDesk kiosk 主窗口（dreamdesk/docs/K-kiosk方案.md 第 2 项）：程序以 `--kiosk` 启动时主窗口换成这一页，全屏。
// 开机问服务端「我绑定哪台」（座位接口，请求由 Rust 那边带上设备签名），以座位身份连上去（要票在 Rust 里自动做）；
// 问不到就显示一句原因、倒计时自动再问，另有「重新连接」「关机」。
// 连上以后掉线、工作站离线由远程窗口自己重试（见 utils/dd_kiosk.dart），窗口不关；远程窗口只在用户断开时关
// （悬浮球的「断开」、提示框里的取消），这时回这一页、不自动再连，等用户点「重新连接」。
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/desktop/pages/desktop_tab_page.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/utils/http_service.dart' as http;
import 'package:flutter_hbb/utils/multi_window_manager.dart';
import 'package:window_manager/window_manager.dart';

class DdKioskPage extends StatefulWidget {
  const DdKioskPage({Key? key}) : super(key: key);

  @override
  State<DdKioskPage> createState() => _DdKioskPageState();
}

class _DdKioskPageState extends State<DdKioskPage> {
  /// 连不上时的重试间隔：从短到长，连上过一次就回到最短
  static const _backoff = [5, 10, 20, 30, 60];
  int _failures = 0;
  String _text = '';
  int _countdown = 0;
  bool _sessionOpen = false;
  bool _busy = false;
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    windowManager.setFullScreen(true);
    // 每秒看一次：远程窗口还在不在、倒计时到没到
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => _onTick());
    _attempt();
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  Future<void> _onTick() async {
    final open = (await rustDeskWinManager.getAllSubWindowIds()).isNotEmpty;
    if (_sessionOpen && !open) {
      debugPrint('dd-kiosk: session window closed by the user');
      _failures = 0;
      setState(() {
        _text = translate('Disconnected');
        _countdown = 0;
      });
    }
    _sessionOpen = open;
    if (!open && !_busy && _countdown > 0) {
      setState(() => _countdown -= 1);
      if (_countdown == 0) _attempt();
    }
  }

  void _scheduleRetry(String reason) {
    final secs = _backoff[_failures.clamp(0, _backoff.length - 1)];
    _failures += 1;
    setState(() {
      _text = reason;
      _countdown = secs;
    });
  }

  Future<void> _attempt() async {
    if (_busy || _sessionOpen) return;
    setState(() {
      _busy = true;
      _countdown = 0;
      _text = translate('Connecting to the workstation...');
    });
    try {
      final api = await bind.mainGetApiServer();
      final resp = await http.get(Uri.parse('$api/api/dd/seat/target'));
      Map<String, dynamic> body = {};
      try {
        body = jsonDecode(resp.body) as Map<String, dynamic>;
      } catch (_) {}
      if (resp.statusCode == 200 && (body['target_id'] ?? '') != '') {
        final id = body['target_id'] as String;
        debugPrint('dd-kiosk: bound to $id, connecting');
        if (mounted) connect(context, id);
        _failures = 0;
        // 远程窗口开出来要一会儿：期间不重复连
        await Future.delayed(const Duration(seconds: 5));
      } else {
        final error = body['error'] as String? ?? '';
        debugPrint('dd-kiosk: seat target ${resp.statusCode}: $error');
        _scheduleRetry(error.isNotEmpty
            ? error
            : translate('This seat is not bound to a workstation yet'));
      }
    } catch (e) {
      debugPrint('dd-kiosk: seat target failed: $e');
      _scheduleRetry(translate('Cannot reach the server'));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final style = Theme.of(context).textTheme;
    // 原来的主窗口页照样建起来、只是不显示：远程窗口要靠它应答主窗口的那套请求（屏幕列表、窗口激活登记等）
    return Stack(children: [
      const Offstage(child: DesktopTabPage()),
      _status(style),
    ]);
  }

  Widget _status(TextTheme style) {
    return Scaffold(
      backgroundColor: Colors.black,
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(_text,
                style: style.headlineSmall?.copyWith(color: Colors.white),
                textAlign: TextAlign.center),
            const SizedBox(height: 16),
            if (_countdown > 0)
              Text(translate('Retrying in {} s').replaceAll('{}', '$_countdown'),
                  style: style.bodyLarge?.copyWith(color: Colors.white70)),
            const SizedBox(height: 32),
            Row(mainAxisSize: MainAxisSize.min, children: [
              ElevatedButton(
                  onPressed: _busy
                      ? null
                      : () {
                          _failures = 0;
                          _attempt();
                        },
                  child: Text(translate('Reconnect'))),
              const SizedBox(width: 24),
              OutlinedButton(
                  onPressed: () => Process.run('systemctl', ['poweroff']),
                  child: Text(translate('Shut down'))),
            ]),
          ],
        ),
      ),
    );
  }
}
