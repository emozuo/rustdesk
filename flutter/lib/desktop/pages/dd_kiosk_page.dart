// DreamDesk kiosk 主窗口（dreamdesk/docs/K-kiosk方案.md）：程序以 `--kiosk` 启动时主窗口换成这一页，全屏。流程照旧版 DreamDesk：
// 开机显示登录页，用 AD 账号登录；登录后列出这个人被授权的工作站（服务端 /api/dd/workstations，带在线、在用），选一台连上
// （要票在 Rust 里按登录的用户自动做）。画面在 widgets/dd_kiosk_view.dart。
// 远程窗口出了连接类错误不自己重试，把原因交过来、关掉自己（utils/dd_kiosk.dart）：掉线这类按退避自动重连同一台，被拒的停在卡片上；
// 远程窗口关了却没交原因的是用户自己断开的（悬浮球的「断开」），回工作站列表。
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/hbbs/hbbs.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/desktop/pages/desktop_tab_page.dart';
import 'package:flutter_hbb/desktop/widgets/dd_icons.dart';
import 'package:flutter_hbb/desktop/widgets/dd_kiosk_view.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/utils/dd_kiosk.dart';
import 'package:flutter_hbb/utils/http_service.dart' as http;
import 'package:flutter_hbb/utils/multi_window_manager.dart';

/// 连接没成的几种情况。auto = 按退避自动重连同一台
enum _Cat {
  lost(true),
  refused(false),
  peer(false);

  final bool auto;
  const _Cat(this.auto);
}

enum _Phase { login, list, connecting, session, card, shuttingDown }

/// 本机记住的上一个用户名（只记用户名，口令不落地）
const _kLastUser = 'dd-kiosk-last-user';
/// 上次连的工作站：回到列表时默认选中它
const _kLastWs = 'dd-kiosk-last-ws';

class DdKioskPage extends StatefulWidget {
  const DdKioskPage({Key? key}) : super(key: key);

  @override
  State<DdKioskPage> createState() => _DdKioskPageState();
}

class _DdKioskPageState extends State<DdKioskPage> {
  /// 掉线后的重试间隔：从短到长；一次连接撑过 [_stableSession] 才断的，从最短重新算
  static const _backoff = [5, 10, 20, 30, 60];
  static const _stableSession = Duration(seconds: 60);
  /// 停在登录页或工作站列表上这么久没人碰：退出登录、清掉口令（公用座位，人走了别人坐下来不能用他的身份）
  static const _idleSignOut = Duration(minutes: 15);
  static const _listRefresh = Duration(seconds: 15);

  _Phase _phase = _Phase.login;
  bool _loginBusy = false;
  String _loginError = '';
  String _lastUser = '';
  String _userName = '';
  String _displayName = '';
  bool _userMenu = false;

  List<DdKioskWsItem> _items = [];
  bool _listLoaded = false;
  String _listError = '';
  String? _selectedId;
  DateTime _lastListFetch = DateTime.fromMillisecondsSinceEpoch(0);

  DdKioskWsItem? _target;
  int _failures = 0;
  _Cat _cat = _Cat.lost;
  String _reason = '';
  int _countdown = 0;
  int _countdownTotal = 1;
  bool _confirm = false;
  bool _sessionOpen = false;
  DateTime? _openedAt;
  // 每次连接一个号：等窗口开出来的那 20 秒里又发起了别的，旧的那次认得出、不再动界面
  int _seq = 0;
  DateTime _lastInput = DateTime.now();
  Timer? _tick;

  String _deviceId = '';
  String _server = '';
  String _version = '';

  final _focus = FocusNode();
  final _loginKey = GlobalKey<DdKioskLoginState>();

  @override
  void initState() {
    super.initState();
    _lastUser = bind.mainGetLocalOption(key: _kLastUser);
    _loadInfo();
    // 每秒看一次：远程窗口还在不在、倒计时到没到、闲置多久了
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => _onTick());
    // 开机总是从登录页开始：上次没退出的登录（断电、程序被杀）先作废
    if (bind.mainGetLocalOption(key: 'access_token').isNotEmpty) _signOut();
  }

  @override
  void dispose() {
    _tick?.cancel();
    _focus.dispose();
    super.dispose();
  }

  Future<void> _loadInfo() async {
    final id = await bind.mainGetMyId();
    final version = await bind.mainGetVersion();
    final api = await bind.mainGetApiServer();
    if (!mounted) return;
    setState(() {
      _deviceId = id;
      _version = version;
      _server = Uri.tryParse(api)?.authority ?? '';
    });
  }

  /// 有没有连接开着：看主窗口的活动窗口名单。远程窗口关掉最后一个连接时只是藏起来留着复用（子窗口还在，不能拿它判断），
  /// 但会通知主窗口把它移出名单；专业模式藏通用窗口不通知，不影响
  bool _sessionWindowOpen() =>
      rustDeskWinManager.getActiveWindows().any((w) => w != kMainWindowId);

  void _onTick() {
    if (!mounted) return;
    final open = _sessionWindowOpen();
    if (_sessionOpen && !open) _onSessionClosed();
    if (!_sessionOpen && open) {
      _openedAt = DateTime.now();
      setState(() => _phase = _Phase.session);
    }
    _sessionOpen = open;
    if (_phase == _Phase.card && !_confirm && _countdown > 0) {
      setState(() => _countdown -= 1);
      if (_countdown == 0) _reconnect();
    }
    if (_phase == _Phase.list && DateTime.now().difference(_lastListFetch) >= _listRefresh) _fetchList();
    final idle = DateTime.now().difference(_lastInput) >= _idleSignOut;
    // 状态卡片也算：人走了、卡片停着（或掉线类一直自己重试），不能让下一个坐下的人接着用上一个人的身份
    if (idle && (_phase == _Phase.list || _phase == _Phase.card)) {
      debugPrint('dd-kiosk: idle, signing out');
      _signOut(message: translate('Signed out after 15 minutes of inactivity.'));
    } else if (idle && _phase == _Phase.login) {
      _lastInput = DateTime.now();
      _loginKey.currentState?.clearPassword();
    }
  }

  void _onSessionClosed() {
    final end = ddKioskLastEnd.value;
    ddKioskLastEnd.value = null;
    final opened = _openedAt;
    // 连上用了一阵才算有人在（远程窗口里的操作这里看不到）：闲置从会话结束算起。自动重试开了又很快关掉的不算，
    // 不然掉线类卡片一直自己重试、永远到不了闲置退出
    if (opened != null && DateTime.now().difference(opened) >= _stableSession) {
      _failures = 0;
      _lastInput = DateTime.now();
    }
    if (end == null) {
      debugPrint('dd-kiosk: session closed by the user');
      _showList();
    } else if (end.manual) {
      debugPrint('dd-kiosk: session ended by the peer: ${end.reason}');
      _show(_Cat.peer, end.reason);
    } else if (end.refused) {
      debugPrint('dd-kiosk: session refused: ${end.reason}');
      _show(_Cat.refused, end.reason);
    } else {
      debugPrint('dd-kiosk: session ended: ${end.reason}');
      _show(_Cat.lost, end.reason);
    }
  }

  /// 显示卡片；会自己再试的排上倒计时
  void _show(_Cat cat, String reason) {
    var secs = 0;
    if (cat.auto) {
      secs = _backoff[_failures.clamp(0, _backoff.length - 1)];
      _failures += 1;
    }
    setState(() {
      _phase = _Phase.card;
      _cat = cat;
      _reason = reason;
      _countdown = secs;
      _countdownTotal = secs == 0 ? 1 : secs;
    });
  }

  Future<void> _login(String user, String password) async {
    setState(() {
      _loginBusy = true;
      _loginError = '';
    });
    try {
      final resp = await gFFI.userModel.login(LoginRequest(
          username: user,
          password: password,
          id: await bind.mainGetMyId(),
          uuid: await bind.mainGetUuid(),
          autoLogin: true,
          type: HttpType.kAuthReqTypeAccount));
      final token = resp.access_token;
      if (resp.type != HttpType.kAuthResTypeToken || token == null) {
        // 服务端要二次验证（邮件 / 动态码）：AD 登录没有这一步，kiosk 也不做
        throw RequestException(0, translate('Sign-in failed'));
      }
      await bind.mainSetLocalOption(key: 'access_token', value: token);
      await bind.mainSetLocalOption(key: _kLastUser, value: user);
      debugPrint('dd-kiosk: signed in as $user');
      if (!mounted) return;
      setState(() {
        _lastUser = user;
        // 网关没给名字（空串）也要有名字：顶栏的用户菜单（退出登录）靠它才出来
        _userName = (resp.user?.name ?? '').isNotEmpty ? resp.user!.name : user;
        _displayName = resp.user?.displayName ?? '';
        _loginBusy = false;
        _items = [];
        _listLoaded = false;
        _listError = '';
        _selectedId = bind.mainGetLocalOption(key: _kLastWs);
      });
      _lastInput = DateTime.now();
      _showList();
    } on RequestException catch (e) {
      debugPrint('dd-kiosk: sign-in failed: ${e.statusCode}');
      _loginFailed(e.cause.isNotEmpty ? translate(e.cause) : translate('Sign-in failed'));
    } catch (e) {
      debugPrint('dd-kiosk: sign-in failed: $e');
      _loginFailed(translate('Cannot reach the server'));
    }
  }

  void _loginFailed(String error) {
    if (!mounted) return;
    setState(() {
      _loginBusy = false;
      _loginError = error;
    });
    _loginKey.currentState?.clearPassword();
  }

  /// 退出登录（用户点、闲置、令牌失效、开机清掉上次的）：先作废服务端的令牌，回不了也照样清本机
  Future<void> _signOut({String message = ''}) async {
    final headers = getHttpHeaders();
    headers['Content-Type'] = 'application/json';
    final api = await bind.mainGetApiServer();
    final body = jsonEncode({'id': await bind.mainGetMyId(), 'uuid': await bind.mainGetUuid()});
    unawaited(http
        .post(Uri.parse('$api/api/logout'), headers: headers, body: body)
        .timeout(const Duration(seconds: 3))
        .then((_) {}, onError: (e) => debugPrint('dd-kiosk: logout request failed: $e')));
    await gFFI.userModel.reset(resetOther: true);
    if (!mounted) return;
    setState(() {
      _phase = _Phase.login;
      _userMenu = false;
      _confirm = false;
      _loginBusy = false;
      _loginError = message;
      _userName = '';
      _displayName = '';
      _items = [];
      _target = null;
    });
  }

  void _showList() {
    setState(() {
      _phase = _Phase.list;
      _confirm = false;
    });
    _fetchList();
  }

  Future<void> _fetchList() async {
    _lastListFetch = DateTime.now();
    final token = bind.mainGetLocalOption(key: 'access_token');
    // 请求在路上时换了人（退出、别人登录）：晚回来的结果是上一个人的，丢掉（不然显示上一个人的工作站，
    // 或者拿上一个人已注销的令牌被拒、把刚登录的人踢下线）
    bool stale() => !mounted || bind.mainGetLocalOption(key: 'access_token') != token;
    try {
      final api = await bind.mainGetApiServer();
      final resp = await http.get(Uri.parse('$api/api/dd/workstations'), headers: getHttpHeaders());
      if (stale()) return;
      if (resp.statusCode == 401) {
        debugPrint('dd-kiosk: token rejected while listing workstations');
        _signOut(message: translate('Your sign-in has expired. Please sign in again.'));
        return;
      }
      final body = jsonDecode(resp.body) as Map<String, dynamic>;
      if (resp.statusCode != 200) {
        setState(() => _listError = body['error'] as String? ?? translate('Could not load your workstations'));
        return;
      }
      final items = <DdKioskWsItem>[];
      for (final m in (body['items'] as List? ?? const [])) {
        if (m is! Map) continue;
        final id = m['id'] as String? ?? '';
        if (id.isEmpty) continue;
        final name = (m['name'] as String?)?.isNotEmpty == true ? m['name'] as String : id;
        final state = m['online'] != true
            ? DdKioskWsState.offline
            : (m['in_use_by_me'] == true
                ? DdKioskWsState.mine
                : (m['in_use'] == true ? DdKioskWsState.busy : DdKioskWsState.available));
        items.add(DdKioskWsItem(id, name, state));
      }
      setState(() {
        _items = items;
        _listLoaded = true;
        _listError = '';
        // 选中的那台没了或者连不了时，换成第一台能连的
        if (!items.any((w) => w.id == _selectedId && w.selectable)) {
          _selectedId = items.where((w) => w.selectable).firstOrNull?.id;
        }
      });
    } catch (e) {
      debugPrint('dd-kiosk: listing workstations failed: $e');
      if (!stale()) setState(() => _listError = translate('Cannot reach the server'));
    }
  }

  void _connectTo(String id) {
    final w = _items.where((w) => w.id == id).firstOrNull;
    if (w == null || !w.selectable) return;
    _failures = 0;
    _target = w;
    bind.mainSetLocalOption(key: _kLastWs, value: id);
    _connect();
  }

  void _reconnect() {
    if (_target == null) return _showList();
    _connect();
  }

  Future<void> _connect() async {
    final w = _target;
    if (w == null || _sessionOpen) return;
    // 退出登录要等几步才切回登录页：期间倒计时到点的重连不能再开出上一个人的会话
    if (bind.mainGetLocalOption(key: 'access_token').isEmpty) return;
    final seq = ++_seq;
    setState(() {
      _phase = _Phase.connecting;
      _countdown = 0;
      _confirm = false;
      _userMenu = false;
    });
    debugPrint('dd-kiosk: connecting to ${w.id}');
    final openedBefore = _openedAt;
    connect(context, w.id);
    // 远程窗口开出来要一会儿：期间不重复连。等够了一次都没开出来（开窗失败），照连不上处理；
    // 开出来又因出错关了的，关窗时已经排了重试
    await Future.delayed(const Duration(seconds: 20));
    if (!mounted || seq != _seq || _openedAt != openedBefore || _sessionWindowOpen()) return;
    // 窗口在两次查看之间开了又关时，它交来的原因还没被取走
    final end = ddKioskLastEnd.value;
    ddKioskLastEnd.value = null;
    debugPrint('dd-kiosk: no session window after connecting: ${end?.reason}');
    if (end == null) {
      _show(_Cat.lost, translate('Connection Error'));
    } else {
      _show(end.manual ? _Cat.peer : (end.refused ? _Cat.refused : _Cat.lost), end.reason);
    }
  }

  Future<void> _shutdown() async {
    setState(() {
      _confirm = false;
      _phase = _Phase.shuttingDown;
    });
    // 起不了进程（找不到 systemctl 等）也要退回去：关机页上没有别的按钮，停在那里只能按电源键
    String failure;
    try {
      final r = await Process.run('systemctl', ['poweroff']);
      if (r.exitCode == 0) return;
      failure = '${r.stderr}';
    } catch (e) {
      failure = '$e';
    }
    debugPrint('dd-kiosk: poweroff failed: $failure');
    if (mounted) setState(() => _phase = _userName.isEmpty ? _Phase.login : _Phase.list);
  }

  bool get _zh {
    final lang = bind.mainGetLocalOption(key: kCommConfKeyLang);
    return lang.isEmpty ? localeName.startsWith('zh') : lang.startsWith('zh');
  }

  void _setLang(bool zh) {
    final key = zh ? 'zh-cn' : 'en';
    bind.mainSetLocalOption(key: kCommConfKeyLang, value: key);
    reloadAllWindows();
    bind.mainChangeLanguage(lang: key);
    setState(() {});
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent e) {
    _lastInput = DateTime.now();
    if (e is! KeyDownEvent) return KeyEventResult.ignored;
    final key = e.logicalKey;
    if (key == LogicalKeyboardKey.escape) {
      if (_confirm || _userMenu) {
        setState(() {
          _confirm = false;
          _userMenu = false;
        });
        return KeyEventResult.handled;
      }
      if (_phase == _Phase.card) {
        _showList();
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    final enter = key == LogicalKeyboardKey.enter || key == LogicalKeyboardKey.numpadEnter;
    if (_confirm) return KeyEventResult.ignored;
    if (_phase == _Phase.card && enter) {
      _failures = 0;
      _reconnect();
      return KeyEventResult.handled;
    }
    if (_phase == _Phase.list) {
      final id = _selectedId;
      if (enter && id != null) {
        _connectTo(id);
        return KeyEventResult.handled;
      }
      // 数字键 1–9 选第几台（旧版是 ⌘1–⌘6）
      final digit = int.tryParse(e.character ?? '');
      if (digit != null && digit >= 1 && digit <= _items.length && _items[digit - 1].selectable) {
        setState(() => _selectedId = _items[digit - 1].id);
        return KeyEventResult.handled;
      }
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final signedIn = _userName.isNotEmpty && _phase != _Phase.login;
    // 原来的主窗口页照样建起来、只是不显示：远程窗口要靠它应答主窗口的那套请求（屏幕列表、窗口激活登记等）
    return Stack(children: [
      const Offstage(child: DesktopTabPage()),
      Listener(
        onPointerDown: (_) => _lastInput = DateTime.now(),
        onPointerHover: (_) => _lastInput = DateTime.now(),
        child: Focus(
          focusNode: _focus,
          autofocus: true,
          onKeyEvent: _onKey,
          child: DdKioskFrame(layers: [
            if (_phase == _Phase.login) _loginView(),
            if (_phase == _Phase.list) _list(),
            if (_phase == _Phase.card) _card(),
            if (_phase == _Phase.connecting)
              DdKioskConnecting(
                kicker: translate('Establishing session'),
                title: translate('Connecting to'),
                target: _target?.name ?? '',
                step: translate('Opening the picture'),
              ),
            if (_phase == _Phase.session) DdKioskConnected(text: '${translate('Connected')} · ${_target?.name ?? ''}'),
            if (_phase != _Phase.shuttingDown) ..._chrome(signedIn),
            if (_confirm)
              DdKioskConfirm(
                title: translate('Shut down this thin client?'),
                text: translate('To use it again, press the power button on the device.'),
                confirmLabel: translate('Shut down'),
                cancelLabel: translate('Cancel'),
                onConfirm: _shutdown,
                onCancel: () => setState(() => _confirm = false),
              ),
            if (_phase == _Phase.shuttingDown) DdKioskShuttingDown(text: translate('Shutting down...')),
          ]),
        ),
      ),
    ]);
  }

  /// 顶栏和底栏放在最后画：用户菜单从顶栏往下展开，要压在页面内容上面
  List<Widget> _chrome(bool signedIn) => [
        DdKioskFooter(
          items: [
            if (_deviceId.isNotEmpty) '${translate('This device')} $_deviceId',
            if (_server.isNotEmpty) '${translate('Server')} $_server',
          ],
          trailing: _phase == _Phase.login || _phase == _Phase.list
              ? DdKioskGhostButton(
                  label: translate('Shut down'),
                  icon: DdIcon.power,
                  fontSize: 11,
                  onTap: () => setState(() => _confirm = true))
              : null,
        ),
        DdKioskTopBar(
          tagline: translate('Ultra-low latency remote desktop'),
          version: _version,
          zh: _zh,
          onLang: _setLang,
          user: signedIn ? (_displayName.isNotEmpty ? _displayName : _userName) : null,
          signedInAs: translate('Signed in as'),
          signOut: translate('Sign out'),
          userMenuOpen: _userMenu,
          onUserTap: () => setState(() => _userMenu = !_userMenu),
          onSignOut: () => _signOut(),
        ),
      ];

  Widget _loginView() => DdKioskLogin(
        key: _loginKey,
        texts: DdKioskLoginTexts(
          kicker: translate('Identity verification'),
          title: translate('Sign in to'),
          user: translate('Username'),
          password: translate('Password'),
          submit: translate('Sign in'),
        ),
        initialUser: _lastUser,
        busy: _loginBusy,
        error: _loginError,
        onSubmit: _login,
      );

  Widget _list() {
    final total = _items.length;
    final ready = _items.where((w) => w.selectable).length;
    final String sub;
    if (_listError.isNotEmpty) {
      sub = _listError;
    } else if (!_listLoaded) {
      sub = translate('Loading...');
    } else if (total == 0) {
      sub = translate('No workstation has been assigned to you yet. Please contact IT.');
    } else {
      // translate 的占位符写成 {值}（src/lang.rs 的 extract_placeholder），写 {} 再替换会被替成空
      sub = '${translate('You have {$total} workstations')} · ${translate('{$ready} available')}';
    }
    return DdKioskWorkstations(
      texts: DdKioskWsTexts(
        kicker: translate('Choose a workstation'),
        titleA: translate('Connect to your'),
        titleB: translate('Dream'),
        sub: sub,
        stateText: (s) => translate(switch (s) {
          DdKioskWsState.available => 'Available',
          DdKioskWsState.mine => 'Connected from elsewhere',
          DdKioskWsState.busy => 'In use by someone else',
          DdKioskWsState.offline => 'Offline',
        }),
        connect: translate('Connect'),
        hint: translate('Number keys to choose · Enter to connect'),
      ),
      items: _items,
      selectedId: _selectedId,
      onSelect: (id) => setState(() => _selectedId = id),
      onConnect: _connectTo,
    );
  }

  Widget _card() {
    final name = _target?.name ?? '';
    final (String kicker, String title, String reason) = switch (_cat) {
      _Cat.lost => (translate('Not connected'), translate('Connection lost'), _reason),
      _Cat.refused => (translate('Connection refused'), translate('Connection refused'), _reason),
      _Cat.peer => (
          translate('Disconnected'),
          translate('Ended from the workstation'),
          translate('Someone at {$name} ended this session.')
        ),
    };
    return DdKioskCard(
      kicker: kicker,
      title: title,
      reason: reason,
      target: name.isNotEmpty ? DdKioskTarget(translate('Workstation'), name, null, '', '') : null,
      countdown: _cat.auto && _countdown > 0
          ? DdKioskCountdown(
              translate('Retrying in {$_countdown} s'),
              translate('Tried {$_failures} times'),
              _countdown / _countdownTotal)
          : null,
      cta: translate(_cat.auto ? 'Reconnect now' : 'Reconnect'),
      onCta: () {
        _failures = 0;
        _reconnect();
      },
      secondaryLabel: translate('Back to list'),
      secondaryIcon: DdIcon.arrowLeft,
      onSecondary: _showList,
    );
  }
}
