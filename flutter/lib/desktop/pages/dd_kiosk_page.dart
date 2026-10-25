// DreamDesk kiosk 主窗口（dreamdesk/docs/K-kiosk方案.md 第 2 项）：程序以 `--kiosk` 启动时主窗口换成这一页，全屏。
// 开机问服务端「我绑定哪台」（座位接口，请求由 Rust 那边带上设备签名），以座位身份连上去（要票在 Rust 里自动做）；
// 问不到就显示一句原因、倒计时自动再问，另有「重新连接」「关机」。
// 重试只在这一页：远程窗口出了连接类错误（掉线、工作站离线、被拒）不自己重试，把原因交过来、关掉自己
// （utils/dd_kiosk.dart），这一页按退避重新问绑定再连——座位改绑了能跟上。远程窗口关了却没交原因的是用户自己断开的
// （悬浮球的「断开」）；被人手动断开的（控制台、工作站上的人）也算。这两种回这一页、不自动再连，等用户点「重新连接」。
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/desktop/pages/desktop_tab_page.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/utils/dd_kiosk.dart';
import 'package:flutter_hbb/utils/http_service.dart' as http;
import 'package:flutter_hbb/utils/multi_window_manager.dart';
import 'package:window_manager/window_manager.dart';

class DdKioskPage extends StatefulWidget {
  const DdKioskPage({Key? key}) : super(key: key);

  @override
  State<DdKioskPage> createState() => _DdKioskPageState();
}

class _DdKioskPageState extends State<DdKioskPage> {
  /// 连不上时的重试间隔：从短到长；一次连接撑过 [_stableSession] 才断的，从最短重新算
  static const _backoff = [5, 10, 20, 30, 60];
  static const _stableSession = Duration(seconds: 60);
  int _failures = 0;
  String _text = '';
  int _countdown = 0;
  bool _sessionOpen = false;
  DateTime? _openedAt;
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

  /// 有没有连接开着：看主窗口的活动窗口名单。远程窗口关掉最后一个连接时只是藏起来留着复用（子窗口还在，不能拿它判断），
  /// 但会通知主窗口把它移出名单；专业模式藏通用窗口不通知，不影响
  bool _sessionWindowOpen() =>
      rustDeskWinManager.getActiveWindows().any((w) => w != kMainWindowId);

  void _onTick() {
    if (!mounted) return;
    final open = _sessionWindowOpen();
    if (_sessionOpen && !open) _onSessionClosed();
    if (!_sessionOpen && open) _openedAt = DateTime.now();
    _sessionOpen = open;
    if (!open && !_busy && _countdown > 0) {
      setState(() => _countdown -= 1);
      if (_countdown == 0) _attempt();
    }
  }

  void _onSessionClosed() {
    final end = ddKioskLastEnd.value;
    ddKioskLastEnd.value = null;
    final opened = _openedAt;
    if (opened != null && DateTime.now().difference(opened) >= _stableSession) {
      _failures = 0;
    }
    if (end == null || end.manual) {
      debugPrint('dd-kiosk: session ended by a person: ${end?.reason}');
      setState(() {
        _text = end?.reason ?? translate('Disconnected');
        _countdown = 0;
      });
    } else {
      debugPrint('dd-kiosk: session ended: ${end.reason}');
      _scheduleRetry(end.reason);
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
        final openedBefore = _openedAt;
        if (mounted) connect(context, id);
        // 远程窗口开出来要一会儿：期间不重复连。等够了一次都没开出来（开窗失败），照连不上处理；
        // 开出来又因出错关了的，关窗时已经排了重试
        await Future.delayed(const Duration(seconds: 20));
        if (mounted && _openedAt == openedBefore && !_sessionWindowOpen()) {
          // 窗口在两次查看之间开了又关时，它交来的原因还没被取走
          final end = ddKioskLastEnd.value;
          ddKioskLastEnd.value = null;
          debugPrint('dd-kiosk: no session window after connecting: ${end?.reason}');
          if (end != null && end.manual) {
            setState(() => _text = end.reason);
          } else {
            _scheduleRetry(end?.reason ?? translate('Connection Error'));
          }
        }
      } else {
        final error = body['error'] as String? ?? '';
        debugPrint('dd-kiosk: seat target ${resp.statusCode}: $error');
        // 服务端没给原因时：404 是没绑定，其余（服务端出错、中间代理回的错误页）按连不上算
        _scheduleRetry(error.isNotEmpty
            ? error
            : translate(resp.statusCode == 404
                ? 'This seat is not bound to a workstation yet'
                : 'Cannot reach the server'));
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
