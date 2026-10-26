// DreamDesk 受管版的主窗口首页（dreamdesk/docs/M-受管版方案.md「桌面端首页」，用户 10-26 定：照网易 UU 远程主窗口的布局和样式）：
// 浅灰的标题栏和左栏连成一片，右边更浅的内容区左上角是圆角。左栏「我的设备」列出网关授权给我的工作站（/api/dd/workstations，
// 带在线、在用）和「全部设备」，左下角「设置」；右边没登录时是插画 +「立即登录」，登录后是全部设备，或一台的详情
// （「进入桌面」大卡片 + 文件传输、终端、端口映射）。只连受管工作站，没有按设备号连接（「远程协助」）。
// 登录框按 UU 的样子另画（只有域账号口令，认证由网关向 AD 做）；设置页就是 RustDesk 的设置页。
// RustDesk 原来的首页（DesktopTabPage）照样建起来、只是不显示：远程窗口要它应答主窗口的那套请求（屏幕列表、窗口激活登记等），
// 设置页也要它登记的东西。
import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart' hide Dialog;
import 'package:flutter_hbb/common/hbbs/hbbs.dart';
import 'package:flutter_hbb/desktop/pages/desktop_setting_page.dart';
import 'package:flutter_hbb/desktop/pages/desktop_tab_page.dart';
import 'package:flutter_hbb/desktop/widgets/dd_icons.dart';
import 'package:flutter_hbb/desktop/widgets/dd_theme.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:flutter_hbb/utils/http_service.dart' as http;
import 'package:get/get.dart';
import 'package:window_manager/window_manager.dart';

// UU 的配色：标题栏和左栏同一个浅灰，内容区更浅，选中项再深一点，主色是蓝
const _kChrome = DdColors.chrome;
const _kPane = DdColors.pane;
const _kSelected = DdColors.selected;
const _kBlue = DdColors.accent;
const _kText = DdColors.ink;
const _kText2 = DdColors.ink2;
const _kText3 = DdColors.ink3;
const _kLine = DdColors.line;
const _kGreen = DdColors.online;

enum _WsState { available, mine, busy, offline }

class _Ws {
  final String id;
  final String name;
  final String os;
  final _WsState state;
  /// 设备详情「设备信息」卡片的内容（网关 /api/dd/workstations 的可选字段，没有的就空、不显示）
  final Map<String, String> info;
  const _Ws(this.id, this.name, this.os, this.state, {this.info = const {}});

  /// 网关给的设备信息字段（10-27 请网关补）：主机名、当前登录的系统账号、IP、DreamDesk 版本、CPU、内存、我最近一次连上的时间
  static const infoKeys = ['hostname', 'os_username', 'ip', 'version', 'cpu', 'memory', 'last_connected'];

  /// 能连：空闲的、或我自己在别处连着的（再连一次接过来）
  bool get connectable => state == _WsState.available || state == _WsState.mine;
  bool get online => state != _WsState.offline;
}

/// 右边显示什么：全部设备、某一台（设备号）、设置
const _kAll = '';
const _kSettings = '\u0000settings';
const _kListRefresh = Duration(seconds: 15);
/// 本机记住的上一个登录名（只记用户名，口令不落地）
const _kLastUser = 'dd-last-user';

/// 单击马上生效，同一个目标在系统双击间隔内再点一次算双击。不用 onDoubleTap：有它在，单击要等约 300 毫秒确认不是双击才生效
/// （点设备显得慢），在标题栏上还会和拖动抢鼠标（拖不动）；RustDesk 标签栏也是这样自己判断双击的
class _Clicks {
  static Object? _target;
  static int _at = 0;

  static bool isDouble(Object target) {
    final now = DateTime.now().millisecondsSinceEpoch;
    final dbl = target == _target && now - _at < bind.getDoubleClickTime();
    _target = dbl ? null : target;
    _at = now;
    return dbl;
  }
}

class DdHomePage extends StatefulWidget {
  const DdHomePage({Key? key}) : super(key: key);

  @override
  State<DdHomePage> createState() => _DdHomePageState();
}

class _DdHomePageState extends State<DdHomePage> {
  String _selected = _kAll;
  SettingsTabKey _settingsTab = SettingsTabKey.general;
  bool _devicesOpen = true;
  List<_Ws> _items = [];
  bool _loaded = false;
  String _error = '';
  /// [_error] 是连不上服务器（网络问题，提示检查网络），不是服务器回的错
  bool _netError = false;
  /// 令牌被拒、登录被清掉了：没登录时的页面写「登录已过期」，而不是普通的登录提示
  bool _expired = false;
  Timer? _timer;
  late final Worker _loginWorker;

  @override
  void initState() {
    super.initState();
    // 登录 / 退出（不管在哪登的：这里、设置页的账户）都重新取列表
    _loginWorker = ever(gFFI.userModel.userName, (_) => _onLoginChanged());
    _timer = Timer.periodic(_kListRefresh, (_) => _fetch());
    WidgetsBinding.instance.addPostFrameCallback((_) => _fetch());
  }

  @override
  void dispose() {
    _loginWorker.dispose();
    _timer?.cancel();
    super.dispose();
  }

  bool get _signedIn => bind.mainGetLocalOption(key: 'access_token').isNotEmpty;

  void _onLoginChanged() {
    if (!mounted) return;
    setState(() {
      _items = [];
      _loaded = false;
      _error = '';
      // 「登录已过期」留到重新登录为止（令牌被拒时登录状态先被清掉，这里会跟着走一遍）
      if (_signedIn) _expired = false;
      if (_selected != _kSettings) _selected = _kAll;
    });
    _fetch();
  }

  void _retry() {
    setState(() {
      _error = '';
      _loaded = false;
    });
    _fetch();
  }

  Future<void> _fetch() async {
    final token = bind.mainGetLocalOption(key: 'access_token');
    if (token.isEmpty) return;
    // 请求在路上时换了人（退出、别人登录）：晚回来的结果是上一个人的，丢掉（不然会显示上一个人的工作站，
    // 或者拿上一个人已注销的令牌被拒、把刚登录的人踢下线）
    bool stale() => !mounted || bind.mainGetLocalOption(key: 'access_token') != token;
    try {
      final api = await bind.mainGetApiServer();
      final resp = await http.get(Uri.parse('$api/api/dd/workstations'), headers: getHttpHeaders());
      if (stale()) return;
      if (resp.statusCode == 401) {
        debugPrint('dd-home: token rejected while listing workstations');
        await gFFI.userModel.reset(resetOther: true);
        if (mounted) setState(() => _expired = true);
        return;
      }
      // 服务器回了话就不是「连不上」：回的不是 JSON（如反向代理的 502 网页）也按服务器出错报，不落到下面的网络错误
      Map<String, dynamic>? parsed;
      try {
        parsed = jsonDecode(resp.body) as Map<String, dynamic>;
      } catch (e) {
        debugPrint('dd-home: workstation list is not JSON (HTTP ${resp.statusCode}): $e');
      }
      final body = parsed;
      if (resp.statusCode != 200 || body == null) {
        setState(() {
          _error = body?['error'] as String? ?? translate('Could not load your workstations');
          _netError = false;
        });
        return;
      }
      final items = <_Ws>[];
      for (final m in (body['items'] as List? ?? const [])) {
        if (m is! Map) continue;
        final id = m['id'] as String? ?? '';
        if (id.isEmpty) continue;
        final name = (m['name'] as String?)?.isNotEmpty == true ? m['name'] as String : id;
        final state = m['online'] != true
            ? _WsState.offline
            : (m['in_use_by_me'] == true
                ? _WsState.mine
                : (m['in_use'] == true ? _WsState.busy : _WsState.available));
        final info = <String, String>{
          for (final k in _Ws.infoKeys)
            if (m[k] is String && (m[k] as String).isNotEmpty) k: m[k] as String,
        };
        items.add(_Ws(id, name, m['os'] as String? ?? '', state, info: info));
      }
      setState(() {
        _items = items;
        _loaded = true;
        _error = '';
        // 选中的那台不在授权里了：回到全部设备
        if (_selected != _kAll && _selected != _kSettings && !items.any((w) => w.id == _selected)) {
          _selected = _kAll;
        }
      });
    } catch (e) {
      debugPrint('dd-home: listing workstations failed: $e');
      if (!stale()) {
        setState(() {
          _error = translate('Cannot reach the server');
          _netError = true;
        });
      }
    }
  }

  void _openSettings(SettingsTabKey tab) => setState(() {
        _selected = _kSettings;
        _settingsTab = tab;
      });

  Future<void> _connect(_Ws w, {bool fileTransfer = false, bool terminal = false, bool tunnel = false}) async {
    if (!w.connectable) return;
    // 远程窗口左上角写的是这台机器的备注名（RustDesk 的 alias）：先记成网关给的工作站名，和用户在列表里点的一致。
    // 网关没给名字时列表里显示的是设备号，那就不记（左上角照旧取主机名）
    if (w.name != w.id && bind.mainGetPeerOptionSync(id: w.id, key: 'alias') != w.name) {
      await bind.mainSetPeerAlias(id: w.id, alias: w.name);
    }
    if (!mounted) return;
    connect(context, w.id, isFileTransfer: fileTransfer, isTerminal: terminal, isTcpTunneling: tunnel);
  }

  @override
  Widget build(BuildContext context) {
    final page = Stack(children: [
      const Offstage(child: DesktopTabPage()),
      // 这一页固定是 UU 的浅色样式，不跟系统深色主题走（设置页照 RustDesk 原样）；点击不要水波纹（UU 没有，显得点了慢半拍），只留悬浮底色
      Theme(
        data: MyTheme.lightTheme.copyWith(splashFactory: NoSplash.splashFactory, highlightColor: Colors.transparent),
        child: Material(
          color: _kChrome,
          child: DefaultTextStyle.merge(
            style: const TextStyle(color: _kText),
            child: Column(children: [
              _TitleBar(onAbout: () => _openSettings(SettingsTabKey.about)),
              Expanded(
                child: Obx(() {
                  // 读一下，登录状态变化时重画
                  gFFI.userModel.userName.value;
                  return Row(children: [
                    SizedBox(width: 236, child: _sidebar()),
                    // 内容区照 UU：上边、左边一条细线和左栏分开，左上角圆角（外层是线的颜色，里层错开 1 像素铺内容区的底）
                    Expanded(
                      child: Container(
                        padding: const EdgeInsets.only(left: 1, top: 1),
                        decoration: const BoxDecoration(
                          color: DdColors.paneLine,
                          borderRadius: BorderRadius.only(topLeft: Radius.circular(10)),
                        ),
                        child: ClipRRect(
                          borderRadius: const BorderRadius.only(topLeft: Radius.circular(9)),
                          child: Container(color: _kPane, child: _right()),
                        ),
                      ),
                    ),
                  ]);
                }),
              ),
            ]),
          ),
        ),
      ),
    ]);
    // 无边框窗口靠这一层拖边缩放（和 RustDesk 主窗口原来的首页一样）
    return isMacOS || kUseCompatibleUiMode
        ? page
        : Obx(() => DragToResizeArea(
              resizeEdgeSize: stateGlobal.resizeEdgeSize.value,
              enableResizeEdges: windowManagerEnableResizeEdges,
              child: page,
            ));
  }

  Widget _sidebar() {
    final signedIn = _signedIn;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      const SizedBox(height: 6),
      _SideRow(
        icon: ddIcon(DdIcon.devices),
        label: translate('My devices'),
        trailing: ddIcon(_devicesOpen ? DdIcon.chevronUp : DdIcon.chevronDown, size: 16, color: _kText2),
        onTap: () => setState(() => _devicesOpen = !_devicesOpen),
      ),
      Expanded(
        child: ListView(padding: EdgeInsets.zero, children: [
          if (_devicesOpen) ...[
            if (signedIn)
              for (final w in _items)
                _SideRow(
                  indent: true,
                  dot: w.online ? _kGreen : null,
                  icon: _OsBadge(os: w.os, online: w.online, size: 20),
                  label: w.name,
                  dim: !w.online,
                  selected: _selected == w.id,
                  onTap: () => setState(() => _selected = w.id),
                  onDoubleTap: () => _connect(w),
                  clickId: w.id,
                ),
            _SideRow(
              indent: true,
              icon: ddAllDevicesIcon(),
              label: translate('All devices'),
              selected: _selected == _kAll,
              onTap: () => setState(() => _selected = _kAll),
            ),
          ],
        ]),
      ),
      // 「设置」上方一条横贯左栏的分隔线（照 UU）
      Container(height: 1, color: DdColors.sideDivider),
      const SizedBox(height: 6),
      _SideRow(
        icon: ddIcon(DdIcon.settings),
        label: translate('Settings'),
        selected: _selected == _kSettings,
        onTap: () => _openSettings(SettingsTabKey.general),
      ),
      const SizedBox(height: 10),
    ]);
  }

  Widget _right() {
    if (_selected == _kSettings) {
      // 设置页照 RustDesk 原样（跟着它自己的主题）
      // 换了要打开的标签（例如从「更多 → 关于」进来）时重建，设置页只在建起来时看 initialTabkey
      return Theme(
          data: Theme.of(Get.context ?? context),
          child: DesktopSettingPage(key: ValueKey(_settingsTab), initialTabkey: _settingsTab));
    }
    if (!_signedIn) return _signInPrompt();
    final w = _items.where((w) => w.id == _selected).firstOrNull;
    return w == null ? _allDevices() : _detail(w);
  }

  Widget _page({required Widget title, required Widget child}) => Padding(
        padding: const EdgeInsets.fromLTRB(40, 30, 40, 24),
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          title,
          const SizedBox(height: 22),
          Expanded(child: child),
        ]),
      );

  Widget _heading(String text) =>
      Text(text, style: const TextStyle(fontSize: 24, fontWeight: FontWeight.w700, color: _kText));

  Widget _signInPrompt() {
    return _page(
      title: _heading(translate('All devices')),
      child: _expired
          ? _StateView(
              icon: ddIcon(DdIcon.lock, size: 40, color: _kText3),
              title: translate('Your sign-in has expired. Please sign in again.'),
              action: _BlueButton(label: translate('Sign in again'), compact: true, onTap: () => _showLogin(context)),
            )
          : Center(
              child: Column(mainAxisSize: MainAxisSize.min, children: [
                const _Illustration(),
                const SizedBox(height: 26),
                Text(translate('Sign in to see the workstations assigned to you.'),
                    style: const TextStyle(fontSize: 15, color: _kText)),
                const SizedBox(height: 18),
                _BlueButton(label: translate('Sign in now'), onTap: () => _showLogin(context)),
              ]),
            ),
    );
  }

  Widget _allDevices() {
    final Widget body;
    // 加载中 / 出错 / 没有授权的工作站：照原型 HomeStates，图标 + 一句话（+ 说明）（+ 按钮）
    if (_error.isNotEmpty) {
      body = _StateView(
        icon: ddIcon(_netError ? DdIcon.wifiOff : DdIcon.circleAlert, size: 40, color: _kText3),
        title: _error,
        subtitle: _netError ? translate('Check the network and try again') : null,
        action: _OutlineButton(label: translate('Retry'), onTap: _retry),
      );
    } else if (!_loaded) {
      body = _StateView(
        icon: const SizedBox(
            width: 24, height: 24, child: CircularProgressIndicator(strokeWidth: 2, color: _kBlue)),
        title: translate('Loading...'),
        dim: true,
      );
    } else if (_items.isEmpty) {
      body = _StateView(
        icon: ddIcon(DdIcon.monitor, size: 40, color: _kText3),
        title: translate('No workstation has been assigned to you yet'),
        subtitle: translate('Please contact IT'),
      );
    } else {
      body = SingleChildScrollView(
        child: Wrap(spacing: 16, runSpacing: 16, children: [
          for (final w in _items)
            _DeviceTile(
              ws: w,
              stateText: _stateText(w.state),
              stateColor: _stateColor(w.state),
              onTap: () => setState(() => _selected = w.id),
            ),
        ]),
      );
    }
    return _page(
      // 标题后面写台数（照原型）
      title: Row(crossAxisAlignment: CrossAxisAlignment.baseline, textBaseline: TextBaseline.alphabetic, children: [
        _heading(translate('All devices')),
        if (_loaded && _error.isEmpty && _items.isNotEmpty) ...[
          const SizedBox(width: 8),
          // 英文写成「1 in total」，不用分单复数
          Text(translate('{${_items.length}} in total'), style: const TextStyle(fontSize: 13, color: _kText2)),
        ],
      ]),
      child: body,
    );
  }

  Widget _detail(_Ws w) {
    final on = w.connectable;
    final actions = [
      (DdIcon.folderUp, translate('File transfer'), () => _connect(w, fileTransfer: true)),
      (DdIcon.terminal, translate('Terminal'), () => _connect(w, terminal: true)),
      (DdIcon.arrowLeftRight, translate('Port mapping'), () => _connect(w, tunnel: true)),
    ];
    return _page(
      title: Row(children: [
        _StatePill(text: _stateText(w.state), color: _stateColor(w.state)),
        const SizedBox(width: 12),
        // 名字后面灰字写系统，「更多」按钮靠右、浅灰底（照原型）
        Expanded(
          child: Row(children: [
            Flexible(child: _heading(w.name)),
            if (_osShort(w.os).isNotEmpty) ...[
              const SizedBox(width: 12),
              Text(_osShort(w.os), style: const TextStyle(fontSize: 12, color: _kText2)),
            ],
          ]),
        ),
        PopupMenuButton<int>(
          tooltip: '',
          offset: const Offset(0, 38),
          color: Colors.white,
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
          // 按钮自己画（RustDesk 的主题不是 Material 3，IconButton 的 style 不生效）
          child: Container(
            width: 32,
            height: 32,
            alignment: Alignment.center,
            decoration: BoxDecoration(color: const Color(0x0F000000), borderRadius: BorderRadius.circular(6)),
            child: ddIcon(DdIcon.moreVertical),
          ),
          itemBuilder: (_) => [
            for (final (i, a) in actions.indexed)
              PopupMenuItem<int>(
                value: i,
                enabled: on,
                height: 36,
                child: Row(children: [
                  ddIcon(a.$1, color: on ? _kText : _kText3),
                  const SizedBox(width: 10),
                  Text(a.$2, style: const TextStyle(fontSize: 14)),
                ]),
              ),
          ],
          onSelected: (i) => actions[i].$3(),
        ),
      ]),
      child: SingleChildScrollView(
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          // 一张卡片：上面是「进入桌面」（能用专业模式时自动是专业模式），下面一条白色的操作栏
          Container(
            decoration: BoxDecoration(
              color: Colors.white,
              borderRadius: BorderRadius.circular(8),
              boxShadow: DdShadow.card,
            ),
            clipBehavior: Clip.antiAlias,
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              SizedBox(
                height: 220,
                child: _EnterDesktop(
                  label: on ? translate('Enter desktop') : _stateText(w.state),
                  arrow: on,
                  os: w.os,
                  online: w.online,
                  onTap: on ? () => _connect(w) : null,
                ),
              ),
              SizedBox(
                height: 52,
                child: Row(children: [
                  for (final (i, a) in actions.indexed) ...[
                    if (i > 0) Container(width: 1, height: 18, color: _kLine),
                    Expanded(child: _ActionCell(icon: a.$1, label: a.$2, onTap: on ? a.$3 : null)),
                  ],
                ]),
              ),
            ]),
          ),
          const SizedBox(height: 18),
          _DeviceInfo(ws: w),
        ]),
      ),
    );
  }

  String _stateText(_WsState s) => translate(switch (s) {
        _WsState.available => 'Online',
        _WsState.mine => 'Connected from elsewhere',
        _WsState.busy => 'In use by someone else',
        _WsState.offline => 'Offline',
      });

  Color _stateColor(_WsState s) => switch (s) {
        _WsState.available => _kGreen,
        _WsState.mine => DdColors.mine,
        _WsState.busy => DdColors.busy,
        _WsState.offline => _kText3,
      };
}

void _showLogin(BuildContext context) {
  showDialog(context: context, barrierColor: DdColors.scrim, builder: (_) => const _LoginDialog());
}

/// 标题栏：左边图标和程序名，右边账户、菜单、最小化 / 关闭（照 UU，没有最大化按钮，双击标题栏照样最大化）。
/// 主窗口是无边框的，原来的标题栏是首页标签栏画的（那一页现在不显示）；按钮沿用 RustDesk 主窗口那组（关闭时藏到托盘等处理都在里面）
class _TitleBar extends StatelessWidget {
  final VoidCallback onAbout;
  const _TitleBar({required this.onAbout});

  @override
  Widget build(BuildContext context) {
    final tabController = Get.find<DesktopTabController>();
    return SizedBox(
      height: 48,
      child: Row(children: [
        Expanded(
          child: GestureDetector(
            behavior: HitTestBehavior.translucent,
            onTap: () {
              if (_Clicks.isDouble(_TitleBar)) toggleMaximize(true).then((v) => stateGlobal.setMaximized(v));
            },
            onPanStart: (_) => startDragging(true),
            child: Row(children: [
              const SizedBox(width: 16),
              loadIcon(22),
              const SizedBox(width: 8),
              const Text('DreamDesk', style: TextStyle(fontSize: 15, fontWeight: FontWeight.w600, color: _kText)),
            ]),
          ),
        ),
        const _AccountButton(),
        _MoreButton(onAbout: onAbout),
        WindowActionPanel(
          isMainWindow: true,
          state: tabController.state,
          tabController: tabController,
          invisibleTabKeys: RxList<String>.empty(),
          showMaximize: false,
        ),
      ]),
    );
  }
}

/// 标题栏的「更多」（照 UU 的白色圆角菜单）：关于、检查更新。
/// 还没有升级分发（K3 / S3），RustDesk 自带的检查更新只问官方 GitHub、定制客户端不用，所以「检查更新」先如实告诉用户
/// 当前版本、新版本由 IT 安装；有了升级分发再接真的检查
class _MoreButton extends StatelessWidget {
  final VoidCallback onAbout;
  const _MoreButton({required this.onAbout});

  @override
  Widget build(BuildContext context) {
    PopupMenuItem<int> item(int value, DdIcon icon, String label) => PopupMenuItem<int>(
          value: value,
          height: 40,
          child: Row(children: [
            ddIcon(icon),
            const SizedBox(width: 12),
            Text(label, style: const TextStyle(fontSize: 14, color: _kText)),
          ]),
        );
    return PopupMenuButton<int>(
      tooltip: translate('More'),
      offset: const Offset(0, 42),
      color: Colors.white,
      elevation: 8,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      icon: ddIcon(DdIcon.menu, size: 20),
      itemBuilder: (_) => [
        item(0, DdIcon.info, translate('About')),
        const PopupMenuDivider(height: 9),
        item(1, DdIcon.arrowUpCircle, translate('Check for updates')),
      ],
      onSelected: (v) => v == 0 ? onAbout() : _checkUpdate(context),
    );
  }

  Future<void> _checkUpdate(BuildContext context) async {
    final version = await bind.mainGetVersion();
    if (!context.mounted) return;
    showDialog(
      context: context,
      builder: (ctx) => Theme(
        data: MyTheme.lightTheme,
        child: AlertDialog(
          backgroundColor: Colors.white,
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
          title: Text(translate('Check for updates'), style: const TextStyle(fontSize: 16, color: _kText)),
          content: Text('${translate('Version')} $version\n\n${translate('Online updates are not available yet. New versions are installed by IT.')}',
              style: const TextStyle(fontSize: 14, color: _kText2, height: 1.6)),
          actions: [_BlueButton(label: translate('OK'), onTap: () => Navigator.of(ctx).pop())],
        ),
      ),
    );
  }
}

/// 标题栏的头像：点开是一张小卡片（照原型 HomeMenus）——没登录：灰头像、一句提示、整宽「立即登录」；
/// 登录了：头像（名字首字）和显示名、账号横排，下面一行「退出登录」
class _AccountButton extends StatelessWidget {
  const _AccountButton();

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<int>(
      tooltip: '',
      offset: const Offset(0, 42),
      color: Colors.white,
      elevation: 8,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      // 登录了是蓝底圆形、写名字首字（照原型），没登录是人像
      icon: Obx(() {
        final name = gFFI.userModel.displayNameOrUserName;
        if (gFFI.userModel.userName.value.isEmpty || name.isEmpty) return ddIcon(DdIcon.user, size: 20);
        return Container(
          width: 24,
          height: 24,
          alignment: Alignment.center,
          decoration: const BoxDecoration(color: _kBlue, shape: BoxShape.circle),
          child: Text(name.characters.first.toUpperCase(), style: const TextStyle(fontSize: 12, color: Colors.white)),
        );
      }),
      itemBuilder: (ctx) => [
        _CardEntry(child: Obx(() {
          final account = gFFI.userModel.userName.value;
          final name = gFFI.userModel.displayNameOrUserName;
          final signedIn = account.isNotEmpty;
          final initial = name.isEmpty ? '' : name.characters.first.toUpperCase();
          // 登录了（照原型）：左边头像、右边名字和账号，下面一行「退出登录」
          if (signedIn) {
            return SizedBox(
              width: 260,
              child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(18, 16, 18, 14),
                  child: Row(children: [
                    Container(
                      width: 44,
                      height: 44,
                      alignment: Alignment.center,
                      decoration: const BoxDecoration(color: _kBlue, shape: BoxShape.circle),
                      child: Text(initial, style: const TextStyle(color: Colors.white, fontSize: 18)),
                    ),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                        Text(name,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600, color: _kText)),
                        if (name != account) ...[
                          const SizedBox(height: 2),
                          Text(account,
                              overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 12, color: _kText2)),
                        ],
                      ]),
                    ),
                  ]),
                ),
                const Divider(height: 1, color: _kLine),
                InkWell(
                  onTap: () {
                    Navigator.of(ctx).pop();
                    gFFI.userModel.logOut();
                  },
                  child: Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                    child: Row(children: [
                      ddIcon(DdIcon.logOut),
                      const SizedBox(width: 10),
                      Text(translate('Sign out'), style: const TextStyle(fontSize: 14, color: _kText)),
                    ]),
                  ),
                ),
              ]),
            );
          }
          return SizedBox(
            width: 220,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 18, 16, 16),
              child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                Center(
                  child: Container(
                    width: 52,
                    height: 52,
                    alignment: Alignment.center,
                    decoration: const BoxDecoration(color: Color(0xFFE4E6EA), shape: BoxShape.circle),
                    child: ddIcon(DdIcon.user, size: 30, color: const Color(0xFFB8BDC5)),
                  ),
                ),
                const SizedBox(height: 12),
                Text(translate('Sign in to see the workstations assigned to you.'),
                    textAlign: TextAlign.center, style: const TextStyle(fontSize: 14, color: _kText, height: 1.5)),
                const SizedBox(height: 16),
                _BlueButton(
                  label: translate('Sign in now'),
                  expand: true,
                  onTap: () {
                    Navigator.of(ctx).pop();
                    _showLogin(context);
                  },
                ),
              ]),
            ),
          );
        })),
      ],
    );
  }
}

/// 弹出菜单里放一张自定义卡片（不是一行菜单项：没有点击效果、不自动关）
class _CardEntry extends PopupMenuEntry<int> {
  final Widget child;
  const _CardEntry({required this.child});

  @override
  double get height => 0;

  @override
  bool represents(int? value) => false;

  @override
  State<_CardEntry> createState() => _CardEntryState();
}

class _CardEntryState extends State<_CardEntry> {
  @override
  Widget build(BuildContext context) => widget.child;
}

/// 左栏的一行（照 UU）：底色块从左栏边上 4 像素开始；选中时浅灰圆角底，蓝竖线在底色块里面、贴最左边（4×16、两头圆、上下居中）
class _SideRow extends StatelessWidget {
  final Widget icon;
  final String label;
  final Widget? trailing;
  final Color? dot;
  final bool selected;
  final bool indent;
  final bool dim;
  final VoidCallback onTap;
  final VoidCallback? onDoubleTap;
  /// 认双击用的：两台工作站可能同名，按设备号认，不然先点一台、马上点另一台会被当成双击
  final String? clickId;
  const _SideRow(
      {required this.icon,
      required this.label,
      this.trailing,
      this.dot,
      this.selected = false,
      this.indent = false,
      this.dim = false,
      required this.onTap,
      this.onDoubleTap,
      this.clickId});

  @override
  Widget build(BuildContext context) {
    // 底色撑满这一行（上下各留 2 像素）；竖线画在底色上面，所以放在 Stack 的后面
    return SizedBox(
      height: 40,
      child: Stack(fit: StackFit.expand, children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(4, 2, 4, 2),
          child: Material(
            color: selected ? _kSelected : Colors.transparent,
            borderRadius: BorderRadius.circular(6),
            child: InkWell(
              borderRadius: BorderRadius.circular(6),
              hoverColor: DdColors.hover,
              onTap: () {
                onTap();
                if (onDoubleTap != null && _Clicks.isDouble('side:${clickId ?? label}')) onDoubleTap!();
              },
              child: Padding(
                padding: EdgeInsets.only(left: indent ? 24 : 14, right: 14),
                child: Row(children: [
                  if (indent)
                    SizedBox(
                      width: 18,
                      child: dot == null
                          ? null
                          : Align(
                              alignment: Alignment.centerLeft,
                              child: Container(
                                  width: 6, height: 6, decoration: BoxDecoration(color: dot, shape: BoxShape.circle)),
                            ),
                    ),
                  icon,
                  const SizedBox(width: 12),
                  Expanded(
                    child: Text(label,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 14, color: dim ? _kText3 : _kText)),
                  ),
                  if (trailing != null) trailing!,
                ]),
              ),
            ),
          ),
        ),
        if (selected)
          Positioned(
            left: 4,
            top: 12,
            bottom: 12,
            child: IgnorePointer(
              child: Container(width: 4, decoration: BoxDecoration(color: _kBlue, borderRadius: BorderRadius.circular(2))),
            ),
          ),
      ]),
    );
  }
}

/// 系统图标：彩色小方块里白色的系统标识（Windows 蓝、Linux 橙、Mac 黑），离线时灰
class _OsBadge extends StatelessWidget {
  final String os;
  final bool online;
  final double size;
  const _OsBadge({required this.os, required this.online, required this.size});

  @override
  Widget build(BuildContext context) {
    final p = os.toLowerCase();
    // 系统标志用 Font Awesome 的品牌图标（dd_icons.dart）
    final (DdIcon icon, Color color) = p.startsWith('linux')
        ? (DdIcon.osLinux, DdColors.osLinux)
        : p.startsWith('mac')
            ? (DdIcon.osMac, DdColors.osMac)
            : (DdIcon.osWindows, DdColors.osWindows);
    return Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: online ? color : DdColors.offline,
        borderRadius: BorderRadius.circular(size * 0.22),
      ),
      child: ddIcon(icon, size: size * 0.6, color: Colors.white),
    );
  }
}

class _StatePill extends StatelessWidget {
  final String text;
  final Color color;
  const _StatePill({required this.text, required this.color});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(8, 3, 10, 3),
      decoration: BoxDecoration(color: DdColors.pill, borderRadius: BorderRadius.circular(DdRadius.pill)),
      child: Row(mainAxisSize: MainAxisSize.min, children: [
        Container(width: 7, height: 7, decoration: BoxDecoration(color: color, shape: BoxShape.circle)),
        const SizedBox(width: 5),
        Text(text, style: const TextStyle(color: Colors.white, fontSize: 12)),
      ]),
    );
  }
}

/// [compact]：小号（高 30、字 13，照原型 dd-btn--compact），放在状态页里
class _BlueButton extends StatelessWidget {
  final String label;
  final VoidCallback? onTap;
  final bool expand;
  final bool compact;
  const _BlueButton({required this.label, this.onTap, this.expand = false, this.compact = false});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: expand ? double.infinity : null,
      height: compact ? 30 : 36,
      child: TextButton(
        onPressed: onTap,
        style: TextButton.styleFrom(
          backgroundColor: onTap == null ? DdColors.disabledFill : _kBlue,
          foregroundColor: Colors.white,
          disabledForegroundColor: Colors.white,
          padding: EdgeInsets.symmetric(horizontal: compact ? 14 : 24),
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(4)),
        ),
        child: Text(label, style: TextStyle(fontSize: compact ? 13 : 14)),
      ),
    );
  }
}

/// 白底细边框的小按钮（照原型 dd-btn--secondary dd-btn--compact），如状态页的「再试」
class _OutlineButton extends StatelessWidget {
  final String label;
  final VoidCallback onTap;
  const _OutlineButton({required this.label, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 30,
      child: OutlinedButton(
        onPressed: onTap,
        style: OutlinedButton.styleFrom(
          backgroundColor: Colors.white,
          foregroundColor: _kText,
          side: const BorderSide(color: _kLine),
          padding: const EdgeInsets.symmetric(horizontal: 14),
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(4)),
        ),
        child: Text(label, style: const TextStyle(fontSize: 13)),
      ),
    );
  }
}

/// 内容区居中的一个状态（照原型 HomeStates）：图标、一句话、（灰色说明）、（按钮），间距 12。[dim]：那句话也用灰色（加载中）
class _StateView extends StatelessWidget {
  final Widget icon;
  final String title;
  final String? subtitle;
  final Widget? action;
  final bool dim;
  const _StateView({required this.icon, required this.title, this.subtitle, this.action, this.dim = false});

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Column(mainAxisSize: MainAxisSize.min, children: [
        icon,
        const SizedBox(height: 12),
        Text(title, textAlign: TextAlign.center, style: TextStyle(fontSize: 14, color: dim ? _kText2 : _kText)),
        if (subtitle != null) ...[
          const SizedBox(height: 12),
          Text(subtitle!, textAlign: TextAlign.center, style: const TextStyle(fontSize: 12, color: _kText2)),
        ],
        if (action != null) ...[const SizedBox(height: 12), action!],
      ]),
    );
  }
}

/// 详情大卡片的上半：远程桌面的「壁纸」（没有远程截图，画一张抽象的风景）+ 居中的「进入桌面 →」
class _EnterDesktop extends StatefulWidget {
  final String label;
  final bool arrow;
  final VoidCallback? onTap;
  final String os;
  final bool online;
  const _EnterDesktop({required this.label, required this.arrow, required this.os, required this.online, this.onTap});

  @override
  State<_EnterDesktop> createState() => _EnterDesktopState();
}

class _EnterDesktopState extends State<_EnterDesktop> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: widget.onTap == null ? SystemMouseCursors.basic : SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Stack(fit: StackFit.expand, children: [
          CustomPaint(painter: _OsWallpaperPainter(os: widget.os, online: widget.online)),
          Container(color: Colors.black.withOpacity(_hover && widget.onTap != null ? 0.12 : 0)),
          // 能连时是白色胶囊按钮（照原型）；连不了时只写状态，不像按钮
          Center(
            child: widget.arrow
                ? Container(
                    height: 44,
                    padding: const EdgeInsets.symmetric(horizontal: 22),
                    decoration: BoxDecoration(
                      color: const Color(0xEBFFFFFF),
                      borderRadius: BorderRadius.circular(22),
                      boxShadow: const [BoxShadow(color: Color(0x26000000), blurRadius: 16, offset: Offset(0, 4))],
                    ),
                    child: Row(mainAxisSize: MainAxisSize.min, children: [
                      Text(widget.label, style: const TextStyle(color: _kText, fontSize: 15, fontWeight: FontWeight.w600)),
                      const SizedBox(width: 8),
                      ddIcon(DdIcon.arrowRight),
                    ]),
                  )
                // 离线时底是浅灰，白字看不清，用深色字
                : Text(widget.label,
                    style: TextStyle(color: widget.online ? Colors.white : _kText2, fontSize: 16, letterSpacing: 1)),
          ),
        ]),
      ),
    );
  }
}

/// 设备卡片、详情大卡片的背景：按被控端系统画的主题背景（10-27 定）。自己画的近似配色——系统自带的壁纸图片有版权、不能打包；
/// 也不取被控端的真实桌面壁纸：公司机器多是统一壁纸认不出来，取图还要上传、有隐私问题。离线一律灰。
/// 数值照原型 dreamdesk/design/prototype/dd.css 的 dd-wall--*（CSS 的线性渐变角度、椭圆光斑的位置和半径都按画布的百分比）
class _OsWallpaperPainter extends CustomPainter {
  final String os;
  final bool online;
  const _OsWallpaperPainter({required this.os, required this.online});

  /// CSS 的 linear-gradient 角度：0 度朝上、顺时针
  static void _linear(Canvas canvas, Rect rect, double deg, List<Color> colors, List<double> stops) {
    final r = deg * math.pi / 180;
    final dx = math.sin(r), dy = -math.cos(r);
    canvas.drawRect(
      rect,
      Paint()
        ..shader = LinearGradient(begin: Alignment(-dx, -dy), end: Alignment(dx, dy), colors: colors, stops: stops)
            .createShader(rect),
    );
  }

  /// CSS 的 radial-gradient(rx% ry% at cx% cy%, color 0, transparent stop)：椭圆光斑，先画单位圆再按两个半径拉伸
  static void _glow(Canvas canvas, Size s, double cx, double cy, double rx, double ry, Color color, double stop) {
    canvas.save();
    canvas.translate(s.width * cx, s.height * cy);
    canvas.scale(s.width * rx, s.height * ry);
    const unit = Rect.fromLTWH(-1, -1, 2, 2);
    canvas.drawCircle(
      Offset.zero,
      1,
      Paint()..shader = RadialGradient(colors: [color, color.withOpacity(0)], stops: [0, stop]).createShader(unit),
    );
    canvas.restore();
  }

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    // 光斑比画布大，不裁会画到下面的白色名字栏、操作栏上
    canvas.clipRect(rect);
    if (!online) {
      _linear(canvas, rect, 135, const [Color(0xFFC9CDD4), Color(0xFFB3B8C0)], const [0, 1]);
      return;
    }
    final p = os.toLowerCase();
    if (p.startsWith('linux')) {
      _linear(canvas, rect, 150, const [Color(0xFF6B2A5A), Color(0xFF3A0F33), Color(0xFF22061E)], const [0, 0.6, 1]);
      _glow(canvas, size, 0.72, 0.82, 0.60, 0.85, const Color(0xFFFF8A3D), 0.62);
    } else if (p.startsWith('mac')) {
      _linear(canvas, rect, 160, const [Color(0xFF24305E), Color(0xFF1A1F3D), Color(0xFF3B2450)], const [0, 0.55, 1]);
      _glow(canvas, size, 0.30, 0.75, 0.60, 0.80, const Color(0xFF7A6CFF), 0.60);
    } else {
      _linear(canvas, rect, 160, const [Color(0xFF0D4BB0), Color(0xFF0A2F78), Color(0xFF071A45)], const [0, 0.55, 1]);
      _glow(canvas, size, 0.42, 0.78, 0.70, 0.90, const Color(0xFF2F7BFF), 0.65);
      _glow(canvas, size, 0.62, 0.62, 0.55, 0.75, const Color(0xFF8FD8FF), 0.60);
    }
  }

  @override
  bool shouldRepaint(covariant _OsWallpaperPainter old) => old.os != os || old.online != online;
}

class _ActionCell extends StatelessWidget {
  final DdIcon icon;
  final String label;
  final VoidCallback? onTap;
  const _ActionCell({required this.icon, required this.label, this.onTap});

  @override
  Widget build(BuildContext context) {
    final color = onTap == null ? _kText3 : _kText;
    return InkWell(
      onTap: onTap,
      child: Row(mainAxisAlignment: MainAxisAlignment.center, children: [
        ddIcon(icon, color: color),
        const SizedBox(width: 8),
        Text(label, style: TextStyle(fontSize: 14, color: color)),
      ]),
    );
  }
}

/// 系统的简称，写在设备卡片状态后面、详情页名字后面：网关给的是「类别 / 版本」（如 windows / Windows 11 Enterprise - 11 (26200)），
/// 取版本、去掉「 - 」后面的版本号
String _osShort(String os) => os.split(' / ').last.split(' - ').first.trim();

/// 设备详情下面的「设备信息」卡片（照原型 HomeDetail）：两列「名称 值」，只列网关给了值的项；IP 后面可以复制
class _DeviceInfo extends StatelessWidget {
  final _Ws ws;
  const _DeviceInfo({required this.ws});

  /// 网关给的时间（RFC3339）按本机时间显示成「年-月-日 时:分」
  static String _time(String s) {
    final t = DateTime.tryParse(s)?.toLocal();
    if (t == null) return s;
    String two(int n) => n.toString().padLeft(2, '0');
    return '${t.year}-${two(t.month)}-${two(t.day)} ${two(t.hour)}:${two(t.minute)}';
  }

  @override
  Widget build(BuildContext context) {
    final i = ws.info;
    final rows = <(String, String, bool)>[
      if (i['ip'] != null) (translate('IP address'), i['ip']!, true),
      // 网关给的系统是「类别 / 版本」（如 windows / Windows 11 Enterprise - 11 (26200)），只显示后面的版本
      if (ws.os.isNotEmpty) (translate('Operating system'), ws.os.split(' / ').last, false),
      if (i['hostname'] != null) (translate('Hostname'), i['hostname']!, false),
      if (i['os_username'] != null) (translate('Signed-in user'), i['os_username']!, false),
      if (i['cpu'] != null) ('CPU', i['cpu']!, false),
      if (i['memory'] != null) (translate('Memory'), i['memory']!, false),
      if (i['version'] != null) (translate('Version'), 'DreamDesk ${i['version']}', false),
      if (i['last_connected'] != null) (translate('Last connected'), _time(i['last_connected']!), false),
    ];
    if (rows.isEmpty) return const SizedBox.shrink();
    Widget label((String, String, bool) r) =>
        Padding(padding: const EdgeInsets.only(right: 12), child: Text(r.$1, style: const TextStyle(fontSize: 13, color: _kText2)));
    Widget value((String, String, bool) r) => Row(children: [
          Flexible(child: Text(r.$2, overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 13, color: _kText))),
          if (r.$3)
            TextButton(
              style: TextButton.styleFrom(
                  minimumSize: const Size(0, 20), padding: const EdgeInsets.symmetric(horizontal: 4), foregroundColor: _kBlue),
              onPressed: () {
                Clipboard.setData(ClipboardData(text: r.$2));
                showToast(translate('Copied'));
              },
              child: Text(translate('Copy'), style: const TextStyle(fontSize: 12)),
            ),
        ]);
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 18),
      decoration: BoxDecoration(color: Colors.white, borderRadius: BorderRadius.circular(8), boxShadow: DdShadow.card),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(translate('Device information'), style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600, color: _kText)),
        const SizedBox(height: 12),
        // 两列，按行往下排；窗口窄（默认 800 宽）时两列放不下 IP，改成一列。
        // 名称列至少 72 宽（照原型），英文名称更长时按最长的那个放宽，同一列的值对齐
        LayoutBuilder(builder: (context, c) {
          final cols = c.maxWidth >= 560 ? 2 : 1;
          const labelWidth = MaxColumnWidth(FixedColumnWidth(72 + 12), IntrinsicColumnWidth());
          final gap = TableRow(children: [for (var j = 0; j < (cols == 2 ? 5 : 2); j++) const SizedBox(height: 12)]);
          return Table(
            defaultVerticalAlignment: TableCellVerticalAlignment.middle,
            columnWidths: cols == 2
                ? const {0: labelWidth, 1: FlexColumnWidth(), 2: FixedColumnWidth(32), 3: labelWidth, 4: FlexColumnWidth()}
                : const {0: labelWidth, 1: FlexColumnWidth()},
            children: [
              for (var k = 0; k < rows.length; k += cols) ...[
                if (k > 0) gap,
                TableRow(children: [
                  label(rows[k]),
                  value(rows[k]),
                  if (cols == 2) ...[
                    const SizedBox.shrink(),
                    if (k + 1 < rows.length) ...[label(rows[k + 1]), value(rows[k + 1])] else ...[
                      const SizedBox.shrink(),
                      const SizedBox.shrink(),
                    ],
                  ],
                ]),
              ],
            ],
          );
        }),
      ]),
    );
  }
}

/// 全部设备里的一张：上面按系统画的主题背景，下面系统图标、名字和状态
class _DeviceTile extends StatelessWidget {
  final _Ws ws;
  final String stateText;
  final Color stateColor;
  final VoidCallback onTap;
  const _DeviceTile({required this.ws, required this.stateText, required this.stateColor, required this.onTap});

  @override
  Widget build(BuildContext context) {
    // 状态行照原型：彩色圆点 + 状态；能连的后面再写系统（「在线 · Windows 11」）
    final os = _osShort(ws.os);
    final state = ws.connectable && os.isNotEmpty ? '$stateText · $os' : stateText;
    return Container(
      width: 220,
      decoration: BoxDecoration(borderRadius: BorderRadius.circular(8), boxShadow: DdShadow.card),
      child: Material(
        color: Colors.white,
        borderRadius: BorderRadius.circular(8),
        elevation: 0,
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            AspectRatio(
              aspectRatio: 2.2,
              child: CustomPaint(painter: _OsWallpaperPainter(os: ws.os, online: ws.online)),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
              child: Row(children: [
                _OsBadge(os: ws.os, online: ws.online, size: 20),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    Text(ws.name,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 14, fontWeight: FontWeight.w600, color: ws.online ? _kText : _kText3)),
                    const SizedBox(height: 2),
                    Row(children: [
                      Container(width: 6, height: 6, decoration: BoxDecoration(color: stateColor, shape: BoxShape.circle)),
                      const SizedBox(width: 5),
                      Flexible(
                        child: Text(state,
                            overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 12, color: _kText2)),
                      ),
                    ]),
                  ]),
                ),
              ]),
            ),
          ]),
        ),
      ),
    );
  }
}

/// 没登录时的插画：一台显示器 + 一部手机，屏幕里是缩略的本程序界面（照 UU 的构图，用色块画）
class _Illustration extends StatelessWidget {
  const _Illustration();

  @override
  Widget build(BuildContext context) {
    Widget screen({required double w, required double h, required double r, required Widget child}) => Container(
          width: w,
          height: h,
          padding: EdgeInsets.all(r * 0.6),
          decoration: BoxDecoration(
            color: const Color(0xFF1F2329),
            borderRadius: BorderRadius.circular(r),
            boxShadow: const [BoxShadow(color: Color(0x22000000), blurRadius: 18, offset: Offset(0, 8))],
          ),
          child: ClipRRect(borderRadius: BorderRadius.circular(r * 0.4), child: child),
        );
    Widget bar(double w, Color c) =>
        Container(width: w, height: 5, margin: const EdgeInsets.only(bottom: 5), decoration: BoxDecoration(color: c, borderRadius: BorderRadius.circular(3)));
    final desk = screen(
      w: 280,
      h: 180,
      r: 8,
      child: Row(children: [
        Container(
          width: 64,
          color: _kChrome,
          padding: const EdgeInsets.fromLTRB(8, 12, 8, 8),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            bar(40, _kText3),
            bar(30, _kBlue),
            bar(36, _kText3),
            bar(26, _kText3),
          ]),
        ),
        Expanded(
          child: Container(
            color: _kPane,
            padding: const EdgeInsets.all(10),
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              bar(60, _kText),
              const SizedBox(height: 4),
              Expanded(
                child: ClipRRect(
                  borderRadius: BorderRadius.circular(4),
                  child: const CustomPaint(painter: _OsWallpaperPainter(os: 'windows', online: true)),
                ),
              ),
              const SizedBox(height: 6),
              Row(children: [for (var i = 0; i < 4; i++) Expanded(child: bar(20, _kLine))]),
            ]),
          ),
        ),
      ]),
    );
    final phone = screen(
      w: 74,
      h: 140,
      r: 12,
      child: Container(
        color: _kPane,
        padding: const EdgeInsets.all(6),
        child: Column(children: [
          bar(30, _kText),
          SizedBox(height: 40, child: ClipRRect(borderRadius: BorderRadius.circular(3), child: const CustomPaint(painter: _OsWallpaperPainter(os: 'windows', online: true), size: Size.infinite))),
          const SizedBox(height: 6),
          Wrap(spacing: 5, runSpacing: 5, children: [
            for (final c in [_kBlue, const Color(0xFFF08A24), _kGreen, const Color(0xFF7FA8FA), _kText3, _kBlue])
              Container(width: 14, height: 14, decoration: BoxDecoration(color: c, borderRadius: BorderRadius.circular(4))),
          ]),
        ]),
      ),
    );
    return SizedBox(
      width: 380,
      height: 230,
      child: Stack(children: [
        Positioned(left: 20, top: 30, child: desk),
        Positioned(right: 20, top: 70, child: phone),
        Positioned(left: 4, top: 18, child: _Spark(color: const Color(0xFFF08A24), size: 12)),
        Positioned(right: 54, top: 10, child: _Spark(color: _kBlue, size: 10)),
        Positioned(right: 0, top: 120, child: _Spark(color: const Color(0xFF8B5CF6), size: 9)),
      ]),
    );
  }
}

class _Spark extends StatelessWidget {
  final Color color;
  final double size;
  const _Spark({required this.color, required this.size});

  @override
  Widget build(BuildContext context) => Transform.rotate(
        angle: math.pi / 4,
        child: Container(width: size, height: size, decoration: BoxDecoration(color: color, borderRadius: BorderRadius.circular(2))),
      );
}

/// 登录框（照 UU 的登录框：浅蓝渐变底、居中标识、下划线输入框、整宽蓝按钮）。只有域账号口令：
/// 交给网关 /api/login，由网关向 AD 验证；本机不认证、不存口令，只存网关发的令牌和用户信息（和 RustDesk 原登录框一样）
class _LoginDialog extends StatefulWidget {
  const _LoginDialog();

  @override
  State<_LoginDialog> createState() => _LoginDialogState();
}

class _LoginDialogState extends State<_LoginDialog> {
  final _user = TextEditingController(text: bind.mainGetLocalOption(key: _kLastUser));
  final _password = TextEditingController();
  bool _busy = false;
  bool _reveal = false;
  String _error = '';

  @override
  void initState() {
    super.initState();
    _user.addListener(() => setState(() {}));
    _password.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _user.dispose();
    _password.dispose();
    super.dispose();
  }

  bool get _ready => !_busy && _user.text.trim().isNotEmpty && _password.text.isNotEmpty;

  Future<void> _submit() async {
    if (!_ready) return;
    final user = _user.text.trim();
    setState(() {
      _busy = true;
      _error = '';
    });
    try {
      final resp = await gFFI.userModel.login(LoginRequest(
          username: user,
          password: _password.text,
          id: await bind.mainGetMyId(),
          uuid: await bind.mainGetUuid(),
          autoLogin: true,
          type: HttpType.kAuthReqTypeAccount));
      final token = resp.access_token;
      if (resp.type != HttpType.kAuthResTypeToken || token == null) {
        // 服务端要二次验证：AD 登录没有这一步
        throw RequestException(0, 'Sign-in failed');
      }
      await bind.mainSetLocalOption(key: 'access_token', value: token);
      await bind.mainSetLocalOption(key: 'user_info', value: jsonEncode(resp.user ?? {}));
      await bind.mainSetLocalOption(key: _kLastUser, value: user);
      // login() 里先改了用户名、首页那时去取列表还没有令牌（取不到就等 15 秒后的定时刷新）：令牌存好后再通知一次
      gFFI.userModel.userName.refresh();
      if (mounted) Navigator.of(context).pop();
    } on RequestException catch (e) {
      _failed(e.cause.isNotEmpty ? translate(e.cause) : translate('Sign-in failed'));
    } catch (e) {
      debugPrint('dd-home: sign-in failed: $e');
      _failed(translate('Cannot reach the server'));
    }
  }

  void _failed(String error) {
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = error;
      _password.clear();
    });
  }

  // 输入框左边一个灰色小图标（照原型：用户名是人像、口令是锁）
  InputDecoration _decoration(String hint, DdIcon icon, {Widget? suffix}) => InputDecoration(
        hintText: hint,
        hintStyle: const TextStyle(color: _kText3, fontSize: 14),
        isDense: true,
        filled: false,
        prefixIcon: Padding(padding: const EdgeInsets.only(right: 10), child: ddIcon(icon, color: _kText3)),
        prefixIconConstraints: const BoxConstraints(minWidth: 0, minHeight: 0),
        contentPadding: const EdgeInsets.symmetric(vertical: 12),
        enabledBorder: const UnderlineInputBorder(borderSide: BorderSide(color: _kLine)),
        focusedBorder: const UnderlineInputBorder(borderSide: BorderSide(color: _kBlue)),
        suffixIcon: suffix,
      );

  @override
  Widget build(BuildContext context) {
    return Theme(
      data: MyTheme.lightTheme,
      child: Dialog(
        backgroundColor: Colors.transparent,
        elevation: 0,
        child: Container(
          width: 420,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(10),
            gradient: const LinearGradient(
              begin: Alignment.topCenter,
              end: Alignment.bottomCenter,
              colors: [Color(0xFFDCE8FF), Colors.white],
              stops: [0, 0.45],
            ),
            boxShadow: DdShadow.dialog,
          ),
          child: Stack(children: [
            Positioned(
              right: 6,
              top: 6,
              child: IconButton(
                icon: ddIcon(DdIcon.close, color: _kText2),
                onPressed: () => Navigator.of(context).pop(),
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(44, 40, 44, 22),
              child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                Row(mainAxisAlignment: MainAxisAlignment.center, children: [
                  loadIcon(30),
                  const SizedBox(width: 10),
                  const Text('DreamDesk', style: TextStyle(fontSize: 24, fontWeight: FontWeight.w700, color: _kText)),
                ]),
                const SizedBox(height: 26),
                Text(translate('Sign in with your domain account'),
                    textAlign: TextAlign.center,
                    style: const TextStyle(fontSize: 16, fontWeight: FontWeight.w600, color: _kText)),
                const SizedBox(height: 14),
                TextField(
                  controller: _user,
                  autofocus: _user.text.isEmpty,
                  style: const TextStyle(fontSize: 14, color: _kText),
                  decoration: _decoration(translate('Username'), DdIcon.user),
                  onSubmitted: (_) => _submit(),
                ),
                const SizedBox(height: 6),
                TextField(
                  controller: _password,
                  autofocus: _user.text.isNotEmpty,
                  obscureText: !_reveal,
                  style: const TextStyle(fontSize: 14, color: _kText),
                  decoration: _decoration(
                    translate('Password'),
                    DdIcon.lock,
                    suffix: IconButton(
                      icon: ddIcon(_reveal ? DdIcon.eye : DdIcon.eyeOff, color: _kText3),
                      onPressed: () => setState(() => _reveal = !_reveal),
                    ),
                  ),
                  onSubmitted: (_) => _submit(),
                ),
                SizedBox(
                  height: 34,
                  child: Align(
                    alignment: Alignment.centerLeft,
                    child: Text(_error, style: const TextStyle(fontSize: 12, color: DdColors.dangerText)),
                  ),
                ),
                _BlueButton(label: translate(_busy ? 'Loading...' : 'Login'), onTap: _ready ? _submit : null, expand: true),
                const SizedBox(height: 28),
                Text('江苏原力数字科技股份有限公司 版权所有 ©${DateTime.now().year}',
                    textAlign: TextAlign.center, style: const TextStyle(fontSize: 11, color: _kText3)),
              ]),
            ),
          ]),
        ),
      ),
    );
  }
}
