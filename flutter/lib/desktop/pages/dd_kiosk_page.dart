// DreamDesk kiosk 主窗口（dreamdesk/docs/K-kiosk方案.md）：程序以 `--kiosk` 启动时主窗口换成这一页，全屏。流程照旧版 DreamDesk：
// 开机显示登录页，用 AD 账号登录；登录后列出这个人被授权的工作站（服务端 /api/dd/workstations，带在线、在用），选一台连上
// （要票在 Rust 里按登录的用户自动做）。画面在 widgets/dd_kiosk_view.dart。
// 远程窗口出了连接类错误不自己重试，把原因交过来、关掉自己（utils/dd_kiosk.dart）：掉线这类按退避自动重连同一台，被拒的停在卡片上；
// 远程窗口关了却没交原因的是用户自己断开的（悬浮球的「断开」），回工作站列表。
// 受管版的桌面主窗口也用这一页（dreamdesk/docs/M-受管版方案.md 第 3 步），和 kiosk 的不同：普通窗口、页面自己画标题栏；
// 记住登录（重开程序不用再登）、不因闲置退出、没有关机；连上以后连接的出错和重连由远程窗口照 RustDesk 原样处理，首页一直停在列表；
// 用户菜单里能进设置。
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/hbbs/hbbs.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/desktop/pages/desktop_setting_page.dart';
import 'package:flutter_hbb/desktop/pages/desktop_tab_page.dart';
import 'package:flutter_hbb/desktop/widgets/dd_kiosk_view.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:flutter_hbb/utils/dd_kiosk.dart';
import 'package:flutter_hbb/utils/http_service.dart' as http;
import 'package:flutter_hbb/utils/multi_window_manager.dart';
import 'package:get/get.dart';

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
  /// 桌面版：设置页盖在首页上
  bool _settings = false;

  bool get _kiosk => kDdKiosk;

  @override
  void initState() {
    super.initState();
    _lastUser = bind.mainGetLocalOption(key: _kLastUser);
    _loadInfo();
    // 每秒看一次：远程窗口还在不在、倒计时到没到、闲置多久了
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => _onTick());
    if (bind.mainGetLocalOption(key: 'access_token').isNotEmpty) {
      if (_kiosk) {
        // kiosk 开机总是从登录页开始：上次没退出的登录（断电、程序被杀）先作废
        _signOut();
      } else {
        _resumeSignIn();
      }
    }
  }

  /// 桌面版接着用上次的登录：用户名从 RustDesk 登录时存下的用户信息里取；令牌失效的话，取列表时服务端回 401、退回登录页
  void _resumeSignIn() {
    try {
      final info = jsonDecode(bind.mainGetLocalOption(key: 'user_info')) as Map<String, dynamic>;
      _userName = info['name'] as String? ?? '';
      _displayName = info['display_name'] as String? ?? '';
    } catch (_) {}
    if (_userName.isEmpty) _userName = _lastUser;
    _selectedId = bind.mainGetLocalOption(key: _kLastWs);
    _phase = _Phase.list;
    WidgetsBinding.instance.addPostFrameCallback((_) => _fetchList());
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
    if (!_kiosk) {
      if (_phase == _Phase.list && DateTime.now().difference(_lastListFetch) >= _listRefresh) _fetchList();
      return;
    }
    final open = _sessionWindowOpen();
    if (_sessionOpen && !open) _onSessionClosed();
    if (!_sessionOpen && open) {
      _openedAt = DateTime.now();
      _lastInput = DateTime.now();
      setState(() => _phase = _Phase.session);
    }
    _sessionOpen = open;
    if (_phase == _Phase.card && !_confirm && _countdown > 0) {
      setState(() => _countdown -= 1);
      if (_countdown == 0) _reconnect();
    }
    if (_phase == _Phase.list && DateTime.now().difference(_lastListFetch) >= _listRefresh) _fetchList();
    final idle = DateTime.now().difference(_lastInput) >= _idleSignOut;
    if (idle && _phase == _Phase.list) {
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
    if (opened != null && DateTime.now().difference(opened) >= _stableSession) {
      _failures = 0;
    }
    _lastInput = DateTime.now();
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
        _userName = resp.user?.name ?? user;
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
    try {
      final api = await bind.mainGetApiServer();
      final resp = await http.get(Uri.parse('$api/api/dd/workstations'), headers: getHttpHeaders());
      if (!mounted) return;
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
      if (mounted) setState(() => _listError = translate('Cannot reach the server'));
    }
  }

  void _connectTo(String id) {
    final w = _items.where((w) => w.id == id).firstOrNull;
    if (w == null || !w.selectable) return;
    bind.mainSetLocalOption(key: _kLastWs, value: id);
    if (!_kiosk) {
      // 桌面版：开远程窗口就完了，出错、重连由远程窗口自己处理；可以同时连几台
      setState(() => _userMenu = false);
      connect(context, id);
      return;
    }
    _failures = 0;
    _target = w;
    _connect();
  }

  void _reconnect() {
    if (_target == null) return _showList();
    _connect();
  }

  Future<void> _connect() async {
    final w = _target;
    if (w == null || _sessionOpen) return;
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
    final r = await Process.run('systemctl', ['poweroff']);
    if (r.exitCode != 0 && mounted) {
      debugPrint('dd-kiosk: poweroff failed: ${r.stderr}');
      setState(() => _phase = _userName.isEmpty ? _Phase.login : _Phase.list);
    }
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
    if (_settings) return KeyEventResult.ignored;
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
    final page = Listener(
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
      );
    // 原来的主窗口页照样建起来、只是不显示：远程窗口要靠它应答主窗口的那套请求（屏幕列表、窗口激活登记等），
    // 设置页也要它登记的东西（服务停没停）
    return Stack(children: [
      const Offstage(child: DesktopTabPage()),
      if (_kiosk)
        page
      else
        Column(children: [
          const _DdTitleBar(),
          Expanded(child: Stack(children: [page, if (_settings) _settingsView()])),
        ]),
    ]);
  }

  Widget _settingsView() => Material(
        child: Column(children: [
          Align(
            alignment: Alignment.centerLeft,
            child: TextButton.icon(
              onPressed: () => setState(() => _settings = false),
              icon: const Icon(Icons.arrow_back, size: 16),
              label: Text(translate('Back to list')),
            ),
          ),
          Expanded(child: DesktopSettingPage(initialTabkey: SettingsTabKey.general)),
        ]),
      );

  /// 顶栏和底栏放在最后画：用户菜单从顶栏往下展开，要压在页面内容上面
  List<Widget> _chrome(bool signedIn) => [
        DdKioskFooter(
          items: [
            if (_deviceId.isNotEmpty) '${translate('This device')} $_deviceId',
            if (_server.isNotEmpty) '${translate('Server')} $_server',
          ],
          trailing: _kiosk && (_phase == _Phase.login || _phase == _Phase.list)
              ? DdKioskGhostButton(
                  label: translate('Shut down'),
                  icon: Icons.power_settings_new,
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
          settings: translate('Settings'),
          onSettings: _kiosk
              ? null
              : () => setState(() {
                    _userMenu = false;
                    _settings = true;
                  }),
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
      sub = '${translate('You have {} workstations').replaceAll('{}', '$total')} · '
          '${translate('{} available').replaceAll('{}', '$ready')}';
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
          translate('Someone at {} ended this session.').replaceAll('{}', name)
        ),
    };
    return DdKioskCard(
      kicker: kicker,
      title: title,
      reason: reason,
      target: name.isNotEmpty ? DdKioskTarget(translate('Workstation'), name, null, '', '') : null,
      countdown: _cat.auto && _countdown > 0
          ? DdKioskCountdown(
              translate('Retrying in {} s').replaceAll('{}', '$_countdown'),
              translate('Tried {} times').replaceAll('{}', '$_failures'),
              _countdown / _countdownTotal)
          : null,
      cta: translate(_cat.auto ? 'Reconnect now' : 'Reconnect'),
      onCta: () {
        _failures = 0;
        _reconnect();
      },
      secondaryLabel: translate('Back to list'),
      secondaryIcon: Icons.arrow_back,
      onSecondary: _showList,
    );
  }
}

/// 桌面版的标题栏：主窗口是无边框的，原来的标题栏是首页标签栏画的（那一页现在不显示）。拖动、双击最大化照标签栏的做法，
/// 最小化 / 最大化 / 关闭沿用 RustDesk 主窗口的那组按钮（关闭时的「藏到托盘」等处理都在里面）
class _DdTitleBar extends StatelessWidget {
  const _DdTitleBar();

  @override
  Widget build(BuildContext context) {
    final tabController = Get.find<DesktopTabController>();
    return Theme(
      data: MyTheme.darkTheme,
      child: Container(
        height: kDesktopRemoteTabBarHeight,
        color: const Color(0xFF05050A),
        child: Row(children: [
          Expanded(
            child: GestureDetector(
              behavior: HitTestBehavior.translucent,
              onDoubleTap: () => toggleMaximize(true).then((v) => stateGlobal.setMaximized(v)),
              onPanStart: (_) => startDragging(true),
              child: const SizedBox.expand(),
            ),
          ),
          WindowActionPanel(
            isMainWindow: true,
            state: tabController.state,
            tabController: tabController,
            invisibleTabKeys: RxList<String>.empty(),
          ),
        ]),
      ),
    );
  }
}
