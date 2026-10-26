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
  const _Ws(this.id, this.name, this.os, this.state);

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
      if (_selected != _kSettings) _selected = _kAll;
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
        if (mounted) setState(() => _error = translate('Your sign-in has expired. Please sign in again.'));
        return;
      }
      final body = jsonDecode(resp.body) as Map<String, dynamic>;
      if (resp.statusCode != 200) {
        setState(() => _error = body['error'] as String? ?? translate('Could not load your workstations'));
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
        items.add(_Ws(id, name, m['os'] as String? ?? '', state));
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
      if (!stale()) setState(() => _error = translate('Cannot reach the server'));
    }
  }

  void _openSettings(SettingsTabKey tab) => setState(() {
        _selected = _kSettings;
        _settingsTab = tab;
      });

  void _connect(_Ws w, {bool fileTransfer = false, bool terminal = false, bool tunnel = false}) {
    if (!w.connectable) return;
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
                    Expanded(
                      child: ClipRRect(
                        borderRadius: const BorderRadius.only(topLeft: Radius.circular(10)),
                        child: Container(color: _kPane, child: _right()),
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
              icon: const _FourDots(size: 20),
              label: translate('All devices'),
              selected: _selected == _kAll,
              onTap: () => setState(() => _selected = _kAll),
            ),
          ],
        ]),
      ),
      Container(height: 1, color: _kLine, margin: const EdgeInsets.symmetric(horizontal: 12)),
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
      child: Center(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          const _Illustration(),
          const SizedBox(height: 26),
          Text(translate('Sign in to see the workstations assigned to you.'),
              style: const TextStyle(fontSize: 15, color: _kText)),
          if (_error.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text(_error, style: const TextStyle(fontSize: 13, color: DdColors.dangerText)),
          ],
          const SizedBox(height: 18),
          _BlueButton(label: translate('Sign in now'), onTap: () => _showLogin(context)),
        ]),
      ),
    );
  }

  Widget _allDevices() {
    final Widget body;
    if (_error.isNotEmpty) {
      body = Center(child: Text(_error, style: const TextStyle(color: _kText2)));
    } else if (!_loaded) {
      body = Center(child: Text(translate('Loading...'), style: const TextStyle(color: _kText2)));
    } else if (_items.isEmpty) {
      body = Center(
          child: Text(translate('No workstation has been assigned to you yet. Please contact IT.'),
              style: const TextStyle(color: _kText2)));
    } else {
      body = SingleChildScrollView(
        child: Wrap(spacing: 16, runSpacing: 16, children: [
          for (final w in _items)
            _DeviceTile(
              ws: w,
              stateText: _stateText(w.state),
              onTap: () => setState(() => _selected = w.id),
            ),
        ]),
      );
    }
    return _page(title: _heading(translate('All devices')), child: body);
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
        Flexible(child: _heading(w.name)),
        const SizedBox(width: 4),
        PopupMenuButton<int>(
          tooltip: '',
          icon: ddIcon(DdIcon.moreVertical, size: 20),
          itemBuilder: (_) => [
            for (final (i, a) in actions.indexed) PopupMenuItem<int>(value: i, enabled: on, child: Text(a.$2)),
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
              AspectRatio(
                aspectRatio: 2.5,
                child: _EnterDesktop(
                  label: on ? translate('Enter desktop') : _stateText(w.state),
                  arrow: on,
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
              loadIcon(20),
              const SizedBox(width: 8),
              const Text('DreamDesk', style: TextStyle(fontSize: 13, color: _kText)),
            ]),
          ),
        ),
        const _AccountButton(),
        _MoreButton(onAbout: onAbout),
        Container(width: 1, height: 16, color: _kLine, margin: const EdgeInsets.symmetric(horizontal: 6)),
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

/// 标题栏的头像：点开是一张小卡片（照 UU）——没登录：灰头像、一句提示、整宽「立即登录」；
/// 登录了：头像（名字首字）、显示名和账号、「退出登录」
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
      icon: ddIcon(DdIcon.user, size: 20),
      itemBuilder: (ctx) => [
        _CardEntry(child: Obx(() {
          final account = gFFI.userModel.userName.value;
          final name = gFFI.userModel.displayNameOrUserName;
          final signedIn = account.isNotEmpty;
          final initial = name.isEmpty ? '' : name.characters.first.toUpperCase();
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
                    decoration: BoxDecoration(color: signedIn ? _kBlue : const Color(0xFFE4E6EA), shape: BoxShape.circle),
                    child: signedIn
                        ? Text(initial, style: const TextStyle(color: Colors.white, fontSize: 22, fontWeight: FontWeight.w600))
                        : const Icon(Icons.person, size: 34, color: Color(0xFFB8BDC5)),
                  ),
                ),
                const SizedBox(height: 12),
                if (signedIn) ...[
                  Text(name, textAlign: TextAlign.center, style: const TextStyle(fontSize: 15, fontWeight: FontWeight.w600, color: _kText)),
                  if (name != account) ...[
                    const SizedBox(height: 2),
                    Text(account, textAlign: TextAlign.center, style: const TextStyle(fontSize: 12, color: _kText2)),
                  ],
                ] else
                  Text(translate('Sign in to see the workstations assigned to you.'),
                      textAlign: TextAlign.center, style: const TextStyle(fontSize: 14, color: _kText, height: 1.5)),
                const SizedBox(height: 16),
                signedIn
                    ? SizedBox(
                        height: 36,
                        child: OutlinedButton(
                          onPressed: () {
                            Navigator.of(ctx).pop();
                            gFFI.userModel.logOut();
                          },
                          style: OutlinedButton.styleFrom(
                            foregroundColor: _kText,
                            side: const BorderSide(color: _kLine),
                            shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(4)),
                          ),
                          child: Text(translate('Sign out')),
                        ),
                      )
                    : _BlueButton(
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

/// 左栏的一行：选中时浅灰圆角底、最左边一条蓝竖线（照 UU）
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
    // 底色撑满这一行（上下各留 2 像素），蓝竖线在这一行里上下居中
    return SizedBox(
      height: 40,
      child: Stack(fit: StackFit.expand, children: [
        if (selected)
          Positioned(
            left: 4,
            top: 10,
            bottom: 10,
            child: Container(width: 3, decoration: BoxDecoration(color: _kBlue, borderRadius: BorderRadius.circular(2))),
          ),
        Padding(
          padding: const EdgeInsets.fromLTRB(10, 2, 10, 2),
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
                padding: EdgeInsets.only(left: indent ? 18 : 8, right: 8),
                child: Row(children: [
                  if (indent)
                    SizedBox(
                      width: 12,
                      child: dot == null
                          ? null
                          : Container(width: 5, height: 5, decoration: BoxDecoration(color: dot, shape: BoxShape.circle)),
                    ),
                  icon,
                  const SizedBox(width: 10),
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
    final (IconData icon, Color color) = p.startsWith('linux')
        ? (Icons.terminal, DdColors.osLinux)
        : p.startsWith('mac')
            ? (Icons.apple, DdColors.osMac)
            : (Icons.window, DdColors.osWindows);
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        color: online ? color : DdColors.offline,
        borderRadius: BorderRadius.circular(size * 0.22),
      ),
      child: Icon(icon, size: size * 0.66, color: Colors.white),
    );
  }
}

/// 「全部设备」的图标：田字排的四个圆点
class _FourDots extends StatelessWidget {
  final double size;
  const _FourDots({required this.size});

  @override
  Widget build(BuildContext context) {
    final d = size * 0.42;
    Widget dot(Color c) => Container(width: d, height: d, decoration: BoxDecoration(color: c, shape: BoxShape.circle));
    return SizedBox(
      width: size,
      height: size,
      child: Column(mainAxisAlignment: MainAxisAlignment.spaceBetween, children: [
        Row(mainAxisAlignment: MainAxisAlignment.spaceBetween, children: [dot(_kBlue), dot(const Color(0xFF7FA8FA))]),
        Row(mainAxisAlignment: MainAxisAlignment.spaceBetween, children: [
          dot(const Color(0xFF7FA8FA)),
          dot(_kBlue),
        ]),
      ]),
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

class _BlueButton extends StatelessWidget {
  final String label;
  final VoidCallback? onTap;
  final bool expand;
  const _BlueButton({required this.label, this.onTap, this.expand = false});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: expand ? double.infinity : null,
      height: 36,
      child: TextButton(
        onPressed: onTap,
        style: TextButton.styleFrom(
          backgroundColor: onTap == null ? DdColors.disabledFill : _kBlue,
          foregroundColor: Colors.white,
          disabledForegroundColor: Colors.white,
          padding: const EdgeInsets.symmetric(horizontal: 24),
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(4)),
        ),
        child: Text(label, style: const TextStyle(fontSize: 14)),
      ),
    );
  }
}

/// 详情大卡片的上半：远程桌面的「壁纸」（没有远程截图，画一张抽象的风景）+ 居中的「进入桌面 →」
class _EnterDesktop extends StatefulWidget {
  final String label;
  final bool arrow;
  final VoidCallback? onTap;
  const _EnterDesktop({required this.label, required this.arrow, this.onTap});

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
          const CustomPaint(painter: _WallpaperPainter()),
          Container(color: Colors.black.withOpacity(_hover && widget.onTap != null ? 0.28 : 0.12)),
          Center(
            child: Row(mainAxisSize: MainAxisSize.min, children: [
              Text(widget.label, style: const TextStyle(color: Colors.white, fontSize: 16, letterSpacing: 1)),
              if (widget.arrow) ...[
                const SizedBox(width: 12),
                ddIcon(DdIcon.arrowRight, color: Colors.white, size: 20),
              ],
            ]),
          ),
        ]),
      ),
    );
  }
}

class _WallpaperPainter extends CustomPainter {
  const _WallpaperPainter();

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    // 色块的圆比画布大，不裁会画到下面的白色名字栏、操作栏上
    canvas.clipRect(rect);
    canvas.drawRect(
      rect,
      Paint()
        ..shader = const LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [Color(0xFF6E8F7A), Color(0xFF3F7F86), Color(0xFF2B5F7C)],
        ).createShader(rect),
    );
    // 几团柔和的色块，像航拍的水和滩
    final blobs = [
      (0.18, 0.30, 0.42, const Color(0x99C9D8B0)),
      (0.55, 0.55, 0.38, const Color(0x8862A89A)),
      (0.82, 0.22, 0.30, const Color(0x77A9C7C4)),
      (0.35, 0.85, 0.34, const Color(0x664F8F7F)),
    ];
    for (final (x, y, r, c) in blobs) {
      final center = Offset(size.width * x, size.height * y);
      final radius = math.max(size.width, size.height) * r;
      canvas.drawCircle(
        center,
        radius,
        Paint()..shader = RadialGradient(colors: [c, c.withOpacity(0)]).createShader(Rect.fromCircle(center: center, radius: radius)),
      );
    }
  }

  @override
  bool shouldRepaint(covariant CustomPainter oldDelegate) => false;
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

/// 全部设备里的一张：上面缩略的「壁纸」，下面系统图标、名字和状态
class _DeviceTile extends StatelessWidget {
  final _Ws ws;
  final String stateText;
  final VoidCallback onTap;
  const _DeviceTile({required this.ws, required this.stateText, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 220,
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
              child: Opacity(opacity: ws.online ? 1 : 0.45, child: const CustomPaint(painter: _WallpaperPainter())),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
              child: Row(children: [
                _OsBadge(os: ws.os, online: ws.online, size: 22),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    Text(ws.name,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600, color: _kText)),
                    const SizedBox(height: 2),
                    Text(stateText, style: TextStyle(fontSize: 12, color: ws.online ? _kText2 : _kText3)),
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
                  child: const CustomPaint(painter: _WallpaperPainter()),
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
          SizedBox(height: 40, child: ClipRRect(borderRadius: BorderRadius.circular(3), child: const CustomPaint(painter: _WallpaperPainter(), size: Size.infinite))),
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

  InputDecoration _decoration(String hint, {Widget? suffix}) => InputDecoration(
        hintText: hint,
        hintStyle: const TextStyle(color: _kText3, fontSize: 14),
        isDense: true,
        filled: false,
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
                  decoration: _decoration(translate('Username')),
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
